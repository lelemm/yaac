//! Terminal sessions, renderer-neutral (docs/13-terminal.md): opening, attaching and resizing
//! through the hub's `hub/terminal/*` methods, the data stream and its credit, reattaching
//! after a reconnect, per-session state for the tab (title, exit code, new-output dot, bell),
//! and the terminal panel under the composer (P338). egui and WPF both draw from here.
//!
//! A [`Session`] is one shell on the target. Side-pane tabs (`TabKind::Terminal`) and panel
//! tabs both point at sessions by id; neither owns the emulator.
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use yaac_core::{ThreadKey, client::Client};
use yaac_protocol::rpc::RpcId;
use yaac_protocol::{StreamId, TargetId, TerminalShell, TerminalShellsResult, attach};

/// Output credit kept granted to the target (docs/13-terminal.md#latency-and-throughput).
pub(crate) const OUTPUT_CREDIT: u64 = 1024 * 1024;
/// Credit is returned in steps of at least this much.
const CREDIT_STEP: u64 = 64 * 1024;
/// No more credit while the emulator has this much output still to parse.
const MAX_BACKLOG: usize = 4 * 1024 * 1024;
/// Without a size from a renderer by then, the shell starts at 80 × 24.
const SIZE_GRACE: Duration = Duration::from_millis(800);
/// How long a bell keeps the tab flashed.
pub(crate) const BELL_FLASH: Duration = Duration::from_millis(600);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SessionState {
    /// Waiting for a size, or for `hub/terminal/open`'s answer.
    Starting,
    Running,
    /// The connection to the target dropped; the shell keeps running there.
    Reconnecting,
    /// "Process exited with code N".
    Exited(i32),
    Failed(String),
}

/// One shell on the target and its emulator.
pub(crate) struct Session {
    pub(crate) id: u64,
    pub(crate) key: ThreadKey,
    pub(crate) term: yaac_term::Terminal,
    /// The emulator's snapshot, brought up to date by whichever renderer draws the session.
    pub(crate) frame: yaac_term::Frame,
    pub(crate) shell: Option<TerminalShell>,
    pub(crate) cwd: String,
    pub(crate) state: SessionState,
    /// Output arrived while the session was not on screen.
    pub(crate) unseen: bool,
    /// The bell rang; the tab flashes until then.
    pub(crate) bell_until: Option<Instant>,
    /// A multi-line paste waiting for the user's confirmation.
    pub(crate) paste_confirm: Option<String>,
    /// The search field is open, with this text.
    pub(crate) search: Option<String>,
    /// Last key typed, for the cursor blink's 30 s limit.
    pub(crate) last_key: Instant,
    /// Screenshots: the grid keeps this size whatever the area (fixtures are recorded at it).
    pub(crate) pinned: Option<(u16, u16)>,
    /// Found running after the window started: attach (and replay) only once it is shown.
    lazy: bool,
    /// Screenshots: drawn as focused even when the window is not (egui drops widget focus
    /// while the window has none).
    pub(crate) shown_focused: bool,
    remote: Option<String>,
    stream: Option<StreamId>,
    input_credit: u64,
    queued: Vec<u8>,
    /// Output received and not yet given back as credit.
    owed: u64,
    size: Option<(u16, u16)>,
    created: Instant,
    open_sent: bool,
    /// The daemon's title (`hub/terminal/title`), when the emulator has none.
    remote_title: Option<String>,
    /// The name the user gave the terminal (P343); it wins over every automatic title.
    pub(crate) name: Option<String>,
    /// A name given before the terminal existed on the target, sent once it does.
    name_unsent: bool,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session").field("id", &self.id).field("state", &self.state).finish_non_exhaustive()
    }
}

impl Session {
    /// The shell's display name ("bash", "PowerShell").
    pub(crate) fn shell_name(&self) -> String {
        self.shell.as_ref().map_or_else(|| "Terminal".to_owned(), |s| s.name.clone())
    }

    /// The worktree folder's name, for the tab and toolbar.
    pub(crate) fn folder(&self) -> &str {
        self.cwd.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or(&self.cwd)
    }

    /// The tab's title: the user's name for it, else the program's title (OSC 0/2), else the
    /// shell's name.
    pub(crate) fn title(&self) -> String {
        self.name.clone().filter(|n| !n.is_empty()).unwrap_or_else(|| self.automatic_title())
    }

    /// The title without the user's name.
    pub(crate) fn automatic_title(&self) -> String {
        self.frame
            .title
            .clone()
            .or_else(|| self.term.title())
            .or_else(|| self.remote_title.clone())
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| self.shell_name())
    }

    pub(crate) fn bell(&self, now: Instant) -> bool {
        self.bell_until.is_some_and(|t| now < t)
    }

    /// The cursor may blink: focused, and a key within the last 30 s.
    pub(crate) fn may_blink(&self, now: Instant) -> bool {
        now.duration_since(self.last_key) < Duration::from_secs(30)
    }

    /// Whether the shell can take input now.
    pub(crate) fn live(&self) -> bool {
        matches!(self.state, SessionState::Running | SessionState::Starting | SessionState::Reconnecting)
    }

    /// The selection as an excerpt for the composer (a fenced `console` block).
    pub(crate) fn excerpt(&self) -> Option<yaac_code::Excerpt> {
        let text = self.term.selection_text()?;
        let source = format!("{} in {}", self.shell_name(), self.folder());
        Some(yaac_code::terminal_excerpt(&source, &text))
    }

    #[cfg_attr(not(test), allow(dead_code, reason = "tests read the size the renderer set"))]
    pub(crate) fn size(&self) -> Option<(u16, u16)> {
        self.size
    }

    fn reset_stream(&mut self) {
        self.stream = None;
        self.input_credit = 0;
        self.owed = 0;
    }
}

/// An inline rename in progress: the session and the field's text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Rename {
    pub(crate) session: u64,
    pub(crate) text: String,
    /// The field should take focus with all its text selected (the first frame).
    pub(crate) fresh: bool,
}

/// The terminal panel under a thread's composer (P338).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Panel {
    pub(crate) tabs: Vec<u64>,
    pub(crate) active: u64,
    pub(crate) open: bool,
    pub(crate) maximised: bool,
}

/// The shells a target offers (`terminal/shells`), default first.
#[derive(Clone, Debug, Default)]
pub(crate) enum Shells {
    #[default]
    Unknown,
    Loading,
    Ready(Vec<TerminalShell>),
    /// Why the list could not be read.
    Failed(#[allow(dead_code, reason = "kept for the menus' hover text")] String),
}

enum Call {
    Shells(TargetId),
    List(TargetId),
    Open(u64),
    Attach(u64),
}

/// Every terminal session of the window.
#[derive(Default)]
pub(crate) struct Terminals {
    sessions: HashMap<u64, Session>,
    streams: HashMap<StreamId, u64>,
    pending: HashMap<RpcId, Call>,
    pub(crate) shells: HashMap<TargetId, Shells>,
    pub(crate) panels: HashMap<ThreadKey, Panel>,
    /// A renderer should focus this session on its next frame.
    pub(crate) focus: Option<u64>,
    /// The composer should take focus (the panel was closed).
    pub(crate) focus_composer: bool,
    /// A tab title being edited in place (P343).
    pub(crate) renaming: Option<Rename>,
    /// Sessions whose tab was closed; ended on the next tick.
    pub(crate) closing: Vec<u64>,
    /// A program put text on the clipboard (OSC 52); the renderer copies it.
    pub(crate) clipboard: Option<String>,
    shown_now: HashSet<u64>,
    shown_last: HashSet<u64>,
    /// Targets whose running terminals were listed since they connected.
    listed: HashSet<TargetId>,
    wake: Option<Arc<dyn Fn() + Send + Sync>>,
    next: u64,
}

impl std::fmt::Debug for Terminals {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Terminals").field("sessions", &self.sessions.len()).finish_non_exhaustive()
    }
}

/// Why terminals cannot open for this thread, if they cannot.
pub(crate) fn unavailable(client: &Client, key: &ThreadKey) -> Option<String> {
    let Some(t) = client.state.target(&key.target) else {
        return Some("Unknown machine".into());
    };
    if !t.connected() {
        return Some("The machine is offline".into());
    }
    let Some(d) = t.entry.state.daemon.as_ref() else {
        return Some("The machine is still connecting".into());
    };
    if !d.terminal {
        return Some(d.terminal_unavailable_reason.clone().unwrap_or_else(|| "This machine's YAAC daemon has no terminal support".into()));
    }
    None
}

/// The folder a thread's shells start in: its worktree, else its project, else its cwd.
pub(crate) fn folder(client: &Client, key: &ThreadKey) -> Option<String> {
    let s = client.state.summary(key)?;
    Some(
        s.worktree
            .as_ref()
            .map(|w| w.path.as_str())
            .or_else(|| client.state.project_of(key).map(|p| p.folder.as_str()))
            .unwrap_or(s.cwd.as_str())
            .to_owned(),
    )
}

impl Terminals {
    /// How sessions wake the window when their output has been parsed.
    pub(crate) fn set_wake(&mut self, wake: Arc<dyn Fn() + Send + Sync>) {
        if self.wake.is_none() {
            self.wake = Some(wake);
        }
    }

    pub(crate) fn get(&self, id: u64) -> Option<&Session> {
        self.sessions.get(&id)
    }

    pub(crate) fn get_mut(&mut self, id: u64) -> Option<&mut Session> {
        self.sessions.get_mut(&id)
    }

    /// A renderer drew this session this frame (clears its new-output dot).
    pub(crate) fn mark_shown(&mut self, id: u64) {
        self.shown_now.insert(id);
        if let Some(s) = self.sessions.get_mut(&id) {
            s.unseen = false;
        }
    }

    /// The target's shells, asked for on first use.
    pub(crate) fn shells(&mut self, client: &mut Client, target: &TargetId) -> &Shells {
        let entry = self.shells.entry(target.clone()).or_default();
        if matches!(entry, Shells::Unknown) {
            *entry = Shells::Loading;
            let rpc = client.preview_request(attach::names::TERMINAL_SHELLS, json!({ "target": target }));
            self.pending.insert(rpc, Call::Shells(target.clone()));
        }
        &self.shells[target]
    }

    /// A new session for a thread; the shell starts once a renderer gives it a size.
    pub(crate) fn create(&mut self, client: &Client, key: &ThreadKey, shell: Option<TerminalShell>) -> Result<u64, String> {
        if let Some(reason) = unavailable(client, key) {
            return Err(reason);
        }
        let cwd = folder(client, key).ok_or("The thread's folder is not known yet")?;
        // The default shell's name comes from `terminal/shells` (now, or when it answers).
        let shell = shell.or_else(|| match self.shells.get(&key.target) {
            Some(Shells::Ready(list)) => list.first().cloned(),
            _ => None,
        });
        Ok(self.insert(key, shell, cwd))
    }

    /// A session with no target behind it (demo screenshots and benchmarks feed it directly).
    pub(crate) fn insert(&mut self, key: &ThreadKey, shell: Option<TerminalShell>, cwd: String) -> u64 {
        self.next += 1;
        let id = self.next;
        let wake = self.wake.clone();
        let term = yaac_term::Terminal::new(80, 24, move || {
            if let Some(w) = &wake {
                w();
            }
        });
        let now = Instant::now();
        self.sessions.insert(
            id,
            Session {
                id,
                key: key.clone(),
                term,
                frame: yaac_term::Frame::default(),
                shell,
                cwd,
                state: SessionState::Starting,
                unseen: false,
                bell_until: None,
                paste_confirm: None,
                search: None,
                last_key: now.checked_sub(Duration::from_secs(60)).unwrap_or(now),
                remote: None,
                stream: None,
                input_credit: 0,
                queued: Vec::new(),
                owed: 0,
                size: None,
                created: now,
                open_sent: false,
                remote_title: None,
                name: None,
                name_unsent: false,
                pinned: None,
                lazy: false,
                shown_focused: false,
            },
        );
        self.focus = Some(id);
        id
    }

    /// Demo and tests: a session that never talks to a target.
    pub(crate) fn detach_demo(&mut self, id: u64) {
        if let Some(s) = self.sessions.get_mut(&id) {
            s.open_sent = true;
            s.state = SessionState::Running;
        }
    }

    fn send_open(&mut self, client: &mut Client, id: u64, restart: bool) {
        // A default-shell session learns the shell's name from `terminal/shells`.
        if let Some(target) = self.sessions.get(&id).filter(|s| s.shell.is_none()).map(|s| s.key.target.clone()) {
            self.shells(client, &target);
        }
        let Some(s) = self.sessions.get_mut(&id) else { return };
        let (cols, rows) = s.size.unwrap_or((80, 24));
        s.open_sent = true;
        s.state = SessionState::Starting;
        let mut params = json!({
            "target": s.key.target,
            "cwd": s.cwd,
            "threadId": s.key.thread,
            "cols": cols,
            "rows": rows,
        });
        if let Some(shell) = &s.shell {
            params["shell"] = json!(shell.path);
        }
        let rpc = client.preview_request(attach::names::TERMINAL_OPEN, params);
        let _ = restart;
        self.pending.insert(rpc, Call::Open(id));
    }

    /// The renderer's grid size for a session; the emulator reflows at once, the shell is told.
    pub(crate) fn resize(&mut self, client: &mut Client, id: u64, cols: u16, rows: u16) {
        let Some(s) = self.sessions.get_mut(&id) else { return };
        let (cols, rows) = (cols.max(2), rows.max(1));
        if s.size == Some((cols, rows)) {
            return;
        }
        s.size = Some((cols, rows));
        s.term.resize(usize::from(cols), usize::from(rows));
        if !s.open_sent {
            self.send_open(client, id, false);
            return;
        }
        if let (Some(remote), SessionState::Running) = (&s.remote, &s.state) {
            let rpc = client.preview_request(
                attach::names::TERMINAL_RESIZE,
                json!({"target": s.key.target, "terminal": remote, "cols": cols, "rows": rows}),
            );
            // Fire and forget: a resize that fails changes nothing the user must see.
            let _ = rpc;
        }
    }

    /// Bytes for the shell (keys, paste, mouse, the emulator's replies). Sent at once, within
    /// the stream's credit; anything beyond waits for credit, in order.
    pub(crate) fn input(&mut self, client: &mut Client, id: u64, bytes: &[u8]) {
        let Some(s) = self.sessions.get_mut(&id) else { return };
        if bytes.is_empty() || !s.live() {
            return;
        }
        s.queued.extend_from_slice(bytes);
        flush(client, s);
    }

    /// A key typed by the user (not a reply): the blink timer restarts.
    pub(crate) fn typed(&mut self, client: &mut Client, id: u64, bytes: &[u8]) {
        if let Some(s) = self.sessions.get_mut(&id) {
            s.last_key = Instant::now();
        }
        self.input(client, id, bytes);
    }

    /// Paste: sent at once, or held for confirmation when several lines go to a shell prompt.
    pub(crate) fn paste(&mut self, client: &mut Client, id: u64, text: &str, confirmed: bool) {
        let Some(s) = self.sessions.get_mut(&id) else { return };
        let paste = s.term.paste(text);
        if paste.confirm && !confirmed {
            s.paste_confirm = Some(text.to_owned());
            return;
        }
        s.paste_confirm = None;
        self.typed(client, id, &paste.bytes);
    }

    /// End a session: the shell is closed on the target.
    pub(crate) fn close(&mut self, client: &mut Client, id: u64) {
        let Some(s) = self.sessions.remove(&id) else { return };
        if let Some(stream) = s.stream {
            self.streams.remove(&stream);
        }
        if let Some(remote) = &s.remote
            && s.live()
        {
            let _ = client.preview_request(attach::names::TERMINAL_CLOSE, json!({"target": s.key.target, "terminal": remote}));
        }
        for panel in self.panels.values_mut() {
            panel.tabs.retain(|t| *t != id);
            if panel.active == id {
                panel.active = panel.tabs.last().copied().unwrap_or(0);
            }
            if panel.tabs.is_empty() {
                panel.open = false;
                panel.maximised = false;
            }
        }
        self.shown_now.remove(&id);
    }

    /// **Restart** after the shell exited (or the terminal was lost): a new shell, same tab.
    pub(crate) fn restart(&mut self, client: &mut Client, id: u64) {
        let Some(s) = self.sessions.get_mut(&id) else { return };
        if s.live() {
            return;
        }
        if let Some(stream) = s.stream.take() {
            self.streams.remove(&stream);
        }
        s.reset_stream();
        s.remote = None;
        s.queued.clear();
        s.term.reset_modes();
        // The tab's name carries over to the new shell.
        s.name_unsent = s.name.is_some();
        self.send_open(client, id, true);
    }

    /// An attach reply for one of ours. Returns false for replies that are not.
    pub(crate) fn on_reply(&mut self, client: &mut Client, rpc: &RpcId, result: Result<Value, String>) -> bool {
        let Some(call) = self.pending.remove(rpc) else { return false };
        match call {
            Call::Shells(target) => {
                let shells = result
                    .and_then(|v| serde_json::from_value::<TerminalShellsResult>(v).map_err(|e| e.to_string()))
                    .map_or_else(Shells::Failed, |r| Shells::Ready(r.shells));
                if let Shells::Ready(list) = &shells
                    && let Some(default) = list.first()
                {
                    for s in self.sessions.values_mut().filter(|s| s.key.target == target && s.shell.is_none()) {
                        s.shell = Some(default.clone());
                    }
                }
                self.shells.insert(target, shells);
            }
            Call::List(target) => {
                let Ok(list) = result.and_then(|v| serde_json::from_value::<yaac_protocol::TerminalListResult>(v).map_err(|e| e.to_string())) else {
                    return true;
                };
                self.adopt(&target, list.terminals);
            }
            Call::Open(id) | Call::Attach(id) => {
                let Some(s) = self.sessions.get_mut(&id) else {
                    // Closed while opening: end the shell we just started.
                    if let Ok(v) = result
                        && let Some(t) = v.get("terminal").and_then(Value::as_str)
                        && let Some(target) = v.get("target").and_then(Value::as_str)
                    {
                        let _ = client.preview_request(attach::names::TERMINAL_CLOSE, json!({"target": target, "terminal": t}));
                    }
                    return true;
                };
                match result {
                    Ok(v) => {
                        let stream = v.get("stream").and_then(|s| serde_json::from_value::<StreamId>(s.clone()).ok());
                        let credit = v.get("initialCredit").and_then(Value::as_u64).unwrap_or(0);
                        if let Some(t) = v.get("terminal").and_then(Value::as_str) {
                            s.remote = Some(t.to_owned());
                        }
                        if let Some(stream) = stream {
                            s.stream = Some(stream);
                            s.input_credit = credit;
                            s.owed = 0;
                            s.state = SessionState::Running;
                            self.streams.insert(stream, id);
                            send_name(client, s);
                            // Output credit starts at zero: grant the whole window now.
                            client.notify(yaac_protocol::methods::names::STREAM_CREDIT, json!({"stream": stream, "bytes": OUTPUT_CREDIT}));
                            flush(client, s);
                            // The size may have changed while the reply was on its way.
                            if let (Some((cols, rows)), Some(remote)) = (s.size, &s.remote) {
                                let _ = client.preview_request(
                                    attach::names::TERMINAL_RESIZE,
                                    json!({"target": s.key.target, "terminal": remote, "cols": cols, "rows": rows}),
                                );
                            }
                        } else {
                            s.state = SessionState::Failed("The machine gave no stream for the terminal".into());
                        }
                    }
                    Err(e) => s.state = SessionState::Failed(e),
                }
            }
        }
        true
    }

    /// Output, notifications, credit, the emulators' replies, reattaching. Runs every frame,
    /// for every session (hidden ones too: they parse, and their credit keeps flowing).
    pub(crate) fn tick(&mut self, client: &mut Client, now: Instant) -> Option<Duration> {
        self.shown_last = std::mem::take(&mut self.shown_now);
        for id in std::mem::take(&mut self.closing) {
            self.close(client, id);
        }
        for (stream, bytes) in client.take_terminal_data() {
            let Some(s) = self.streams.get(&stream).and_then(|id| self.sessions.get_mut(id)) else { continue };
            s.owed += bytes.len() as u64;
            s.term.feed(&bytes);
            if !self.shown_last.contains(&s.id) {
                s.unseen = true;
            }
        }
        for (method, params) in client.take_terminal_notes() {
            self.note(client, &method, &params);
        }
        self.list_running(client);
        let mut next: Option<Duration> = None;
        let mut soonest = |d: Duration| next = Some(next.map_or(d, |n| n.min(d)));
        let ids: Vec<u64> = self.sessions.keys().copied().collect();
        for id in ids {
            let mut open = false;
            let mut attach = false;
            {
                let s = self.sessions.get_mut(&id).expect("listed above");
                let ev = s.term.take_events();
                if ev.bells > 0 {
                    s.bell_until = Some(now + BELL_FLASH);
                }
                if let Some(text) = ev.clipboard {
                    self.clipboard = Some(text);
                }
                if !ev.input.is_empty() && s.live() {
                    s.queued.extend_from_slice(&ev.input);
                    flush(client, s);
                }
                if let Some(t) = s.bell_until {
                    if now < t {
                        soonest(t - now);
                    } else {
                        s.bell_until = None;
                    }
                }
                // Give back credit for parsed output.
                if let Some(stream) = s.stream
                    && s.owed > 0
                    && (s.owed >= CREDIT_STEP || s.term.idle())
                    && s.term.backlog() < MAX_BACKLOG
                {
                    client.notify(yaac_protocol::methods::names::STREAM_CREDIT, json!({"stream": stream, "bytes": s.owed}));
                    s.owed = 0;
                }
                let connected = client.state.target(&s.key.target).is_some_and(|t| t.connected());
                match s.state {
                    SessionState::Starting if !s.open_sent => {
                        let wait = s.created + SIZE_GRACE;
                        if now >= wait {
                            open = true;
                        } else {
                            soonest(wait - now);
                        }
                    }
                    SessionState::Running | SessionState::Starting if !connected && s.open_sent && s.remote.is_some() => {
                        s.state = SessionState::Reconnecting;
                        if let Some(stream) = s.stream.take() {
                            self.streams.remove(&stream);
                        }
                        s.reset_stream();
                    }
                    SessionState::Reconnecting if connected && (!s.lazy || self.shown_last.contains(&id)) => attach = true,
                    _ => {}
                }
            }
            if open {
                self.send_open(client, id, false);
            }
            if attach {
                self.reattach(client, id);
            }
        }
        next
    }

    /// Ask each newly connected target which terminals run there (a window that comes back
    /// finds the shells it had, P332).
    fn list_running(&mut self, client: &mut Client) {
        let connected: Vec<TargetId> = client
            .state
            .targets
            .iter()
            .filter(|t| t.connected() && t.entry.state.daemon.as_ref().is_some_and(|d| d.terminal))
            .map(|t| t.entry.id.clone())
            .collect();
        self.listed.retain(|t| connected.contains(t));
        for target in connected {
            if self.listed.insert(target.clone()) {
                let rpc = client.preview_request(attach::names::TERMINAL_LIST, json!({ "target": target }));
                self.pending.insert(rpc, Call::List(target));
            }
        }
    }

    /// Terminals running on a target that no session here shows: each becomes a tab in its
    /// thread's terminal panel (left closed), attached when first shown.
    pub(crate) fn adopt(&mut self, target: &TargetId, running: Vec<yaac_protocol::TerminalInfo>) {
        for info in running {
            let known = self.sessions.values().any(|s| &s.key.target == target && s.remote.as_deref() == Some(info.terminal.as_str()));
            let Some(thread) = info.thread_id.clone() else { continue };
            if known {
                continue;
            }
            let key = ThreadKey::new(target.clone(), thread);
            let name = info.shell.rsplit(['/', '\\']).next().unwrap_or(&info.shell).trim_end_matches(".exe").to_owned();
            let shell = Some(TerminalShell { path: info.shell.clone(), name });
            let focus = self.focus;
            let id = self.insert(&key, shell, info.cwd.clone());
            self.focus = focus;
            let s = self.sessions.get_mut(&id).expect("just inserted");
            s.remote = Some(info.terminal);
            s.remote_title = Some(info.title).filter(|t| !t.is_empty());
            s.name = info.name.filter(|n| !n.is_empty());
            s.open_sent = true;
            s.state = SessionState::Reconnecting;
            s.lazy = true;
            let p = self.panels.entry(key).or_default();
            p.tabs.push(id);
            if p.active == 0 {
                p.active = id;
            }
        }
    }

    /// The connection is back: attach to the same terminal. The target replays its scrollback
    /// into a fresh emulator, so nothing is shown twice.
    fn reattach(&mut self, client: &mut Client, id: u64) {
        let wake = self.wake.clone();
        let Some(s) = self.sessions.get_mut(&id) else { return };
        let Some(remote) = s.remote.clone() else { return };
        let (cols, rows) = s.size.unwrap_or((80, 24));
        s.term = yaac_term::Terminal::new(usize::from(cols), usize::from(rows), move || {
            if let Some(w) = &wake {
                w();
            }
        });
        s.frame = yaac_term::Frame::default();
        s.state = SessionState::Starting;
        s.lazy = false;
        let rpc = client.preview_request(
            attach::names::TERMINAL_ATTACH,
            json!({"target": s.key.target, "terminal": remote, "cols": cols, "rows": rows}),
        );
        self.pending.insert(rpc, Call::Attach(id));
    }

    fn note(&mut self, client: &mut Client, method: &str, params: &Value) {
        if method == yaac_protocol::methods::names::STREAM_CREDIT {
            let Some(stream) = params.get("stream").and_then(|s| serde_json::from_value::<StreamId>(s.clone()).ok()) else { return };
            if let Some(s) = self.streams.get(&stream).and_then(|id| self.sessions.get_mut(id)) {
                s.input_credit = s.input_credit.saturating_add(params.get("bytes").and_then(Value::as_u64).unwrap_or(0));
                flush(client, s);
            }
            return;
        }
        let (Some(target), Some(terminal)) = (params.get("target").and_then(Value::as_str), params.get("terminal").and_then(Value::as_str)) else {
            return;
        };
        let Some(s) = self
            .sessions
            .values_mut()
            .find(|s| s.key.target.as_str() == target && s.remote.as_deref() == Some(terminal))
        else {
            return;
        };
        if method == attach::names::TERMINAL_EXITED {
            let code = params.get("code").and_then(Value::as_i64).unwrap_or(0);
            s.state = SessionState::Exited(i32::try_from(code).unwrap_or(-1));
            s.queued.clear();
            if let Some(stream) = s.stream.take() {
                self.streams.remove(&stream);
            }
            s.reset_stream();
        } else if method == attach::names::TERMINAL_RENAMED {
            s.name = params.get("name").and_then(Value::as_str).map(str::to_owned).filter(|n| !n.is_empty());
        } else if method == attach::names::TERMINAL_TITLE {
            s.remote_title = params.get("title").and_then(Value::as_str).map(str::to_owned);
        }
    }

    // ---- renaming (P343) ------------------------------------------------------------------

    /// Start editing a tab's title in place (double click, F2, the menu's Rename).
    pub(crate) fn begin_rename(&mut self, id: u64) {
        if let Some(s) = self.sessions.get(&id) {
            self.renaming = Some(Rename { session: id, text: s.title(), fresh: true });
        }
    }

    /// Esc, or a click away.
    pub(crate) fn cancel_rename(&mut self) {
        self.renaming = None;
    }

    /// Enter: keep the name. Empty goes back to the automatic title; text equal to the
    /// automatic title of an unnamed terminal changes nothing.
    pub(crate) fn commit_rename(&mut self, client: &mut Client) {
        let Some(r) = self.renaming.take() else { return };
        self.rename(client, r.session, &r.text);
    }

    /// Name a terminal (P343): trimmed; empty goes back to the automatic title; the automatic
    /// title itself on an unnamed terminal changes nothing. Kept on the target when it can.
    pub(crate) fn rename(&mut self, client: &mut Client, id: u64, text: &str) {
        if self.renaming.as_ref().is_some_and(|r| r.session == id) {
            self.renaming = None;
        }
        let Some(s) = self.sessions.get_mut(&id) else { return };
        let text = text.trim().to_owned();
        let name = (!text.is_empty()).then_some(text);
        if name == s.name || (s.name.is_none() && name.as_deref() == Some(s.automatic_title().as_str())) {
            return;
        }
        s.name = name;
        s.name_unsent = true;
        send_name(client, s);
    }

    // ---- the panel under the composer (P338) --------------------------------------------

    pub(crate) fn panel(&self, key: &ThreadKey) -> Option<&Panel> {
        self.panels.get(key)
    }

    /// Whether the panel is showing for this thread.
    pub(crate) fn panel_open(&self, key: &ThreadKey) -> bool {
        self.panels.get(key).is_some_and(|p| p.open && !p.tabs.is_empty())
    }

    /// **Toggle terminal panel** (Ctrl+`): opening starts a terminal when there is none and
    /// focuses the active one; closing hands focus back to the composer.
    pub(crate) fn toggle_panel(&mut self, client: &Client, key: &ThreadKey) -> Result<bool, String> {
        if self.panel_open(key) {
            let p = self.panels.get_mut(key).expect("open panel");
            p.open = false;
            self.focus_composer = true;
            return Ok(false);
        }
        let empty = self.panels.get(key).is_none_or(|p| p.tabs.is_empty());
        if empty {
            self.panel_new(client, key, None)?;
        } else {
            let p = self.panels.get_mut(key).expect("panel with tabs");
            p.open = true;
            self.focus = Some(p.active);
        }
        Ok(true)
    }

    /// **New terminal**: a tab in the panel (opening it).
    pub(crate) fn panel_new(&mut self, client: &Client, key: &ThreadKey, shell: Option<TerminalShell>) -> Result<u64, String> {
        let id = self.create(client, key, shell)?;
        let p = self.panels.entry(key.clone()).or_default();
        p.tabs.push(id);
        p.active = id;
        p.open = true;
        self.focus = Some(id);
        Ok(id)
    }

    pub(crate) fn panel_select(&mut self, key: &ThreadKey, id: u64) {
        if let Some(p) = self.panels.get_mut(key).filter(|p| p.tabs.contains(&id)) {
            p.active = id;
            self.focus = Some(id);
        }
    }

    /// **Next / Previous terminal** in the panel.
    pub(crate) fn panel_cycle(&mut self, key: &ThreadKey, forward: bool) {
        let Some(p) = self.panels.get_mut(key) else { return };
        let Some(i) = p.tabs.iter().position(|t| *t == p.active) else { return };
        let n = p.tabs.len();
        let j = if forward { (i + 1) % n } else { (i + n - 1) % n };
        p.active = p.tabs[j];
        self.focus = Some(p.active);
    }

    /// **Maximise terminal panel** / Restore.
    pub(crate) fn panel_maximise(&mut self, key: &ThreadKey) {
        if let Some(p) = self.panels.get_mut(key).filter(|p| p.open) {
            p.maximised = !p.maximised;
        }
    }
}

/// Tell the target the user's name for a terminal, once it exists there.
fn send_name(client: &mut Client, s: &mut Session) {
    let Some(remote) = &s.remote else { return };
    if !s.name_unsent || !matches!(s.state, SessionState::Running | SessionState::Starting) {
        return;
    }
    let mut params = json!({"target": s.key.target, "terminal": remote});
    if let Some(name) = &s.name {
        params["name"] = json!(name);
    }
    // An older yaacd cannot keep names: the window keeps it, so nothing more to do on failure.
    let _ = client.preview_request(attach::names::TERMINAL_RENAME, params);
    s.name_unsent = false;
}

/// Send what the credit allows.
fn flush(client: &mut Client, s: &mut Session) {
    let Some(stream) = s.stream else { return };
    if s.queued.is_empty() || s.input_credit == 0 {
        return;
    }
    let n = usize::try_from(s.input_credit).unwrap_or(usize::MAX).min(s.queued.len());
    let rest = s.queued.split_off(n);
    let bytes = std::mem::replace(&mut s.queued, rest);
    s.input_credit -= bytes.len() as u64;
    client.send_terminal_data(stream, bytes);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview_controller::Browsers;
    use yaac_core::demo::{DemoHub, Scenario, threads};

    fn rig() -> (Client, DemoHub, Browsers, ThreadKey) {
        let (mut client, hub) = crate::app::demo_client(Scenario::Fleet);
        let key = threads::orchestrator();
        let t = client.state.targets.iter_mut().find(|t| t.entry.id == key.target).unwrap();
        t.entry.state.daemon = Some(yaac_protocol::attach::DaemonSummary {
            build: "test".into(),
            older: false,
            schema_hash_matches: true,
            can_update: false,
            port_forward: true,
            file_view: true,
            terminal: true,
            terminal_unavailable_reason: None,
        });
        (client, hub, Browsers::default(), key)
    }

    /// Hub round trips, then the window's tick.
    fn turn(client: &mut Client, b: &mut Browsers) {
        for _ in 0..3 {
            client.pump();
            b.tick(client, None, Instant::now());
        }
    }

    fn settle(b: &Browsers, id: u64) {
        while !b.terms.get(id).unwrap().term.idle() {
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn screen(b: &mut Browsers, id: u64) -> Vec<String> {
        settle(b, id);
        let s = b.terms.get_mut(id).unwrap();
        s.term.snapshot(&mut s.frame);
        s.frame.lines.iter().map(|r| r.text().trim_end().to_owned()).collect()
    }

    fn opened(hub: &DemoHub) -> Vec<Value> {
        hub.requests().into_iter().filter(|(_, m, _)| m == attach::names::TERMINAL_OPEN).map(|(_, _, p)| p).collect()
    }

    #[test]
    fn a_tab_opens_its_shell_at_the_renderers_size_in_the_worktree() {
        let (mut client, hub, mut b, key) = rig();
        let tab = b.open_terminal(&client, &key, None).unwrap();
        let TabKindTerminal(id) = kind(&b, &key, tab);
        turn(&mut client, &mut b);
        assert!(opened(&hub).is_empty(), "no shell before a renderer gives a size");
        b.terms.resize(&mut client, id, 100, 30);
        let open = opened(&hub);
        assert_eq!(open.len(), 1);
        assert_eq!((open[0]["cols"].as_u64(), open[0]["rows"].as_u64()), (Some(100), Some(30)));
        assert_eq!(open[0]["cwd"].as_str(), folder(&client, &key).as_deref());
        assert_eq!(open[0]["threadId"], json!(key.thread));
        turn(&mut client, &mut b);
        let s = b.terms.get(id).unwrap();
        assert_eq!(s.state, SessionState::Running);
        assert_eq!(s.shell_name(), "bash", "the default shell's name, from terminal/shells");
        // Output credit starts at zero: the window grants 1 MiB at once.
        let credit = hub.notifications().into_iter().find(|(m, _)| m == "stream/credit").unwrap().1;
        assert_eq!(credit["bytes"].as_u64(), Some(OUTPUT_CREDIT));
        assert!(screen(&mut b, id).iter().any(|l| l.starts_with("dev@demo:~/work$")));
    }

    /// The session behind a Terminal tab.
    struct TabKindTerminal(u64);
    fn kind(b: &Browsers, key: &ThreadKey, tab: u64) -> TabKindTerminal {
        match b.tab(key, tab).unwrap().kind {
            crate::file_pane::TabKind::Terminal(id) => TabKindTerminal(id),
            ref k => panic!("{k:?}"),
        }
    }

    #[test]
    fn keys_typed_before_the_stream_are_sent_in_order_and_echoed() {
        let (mut client, hub, mut b, key) = rig();
        let id = b.new_panel_terminal(&client, &key, None).unwrap();
        b.terms.typed(&mut client, id, b"ec");
        b.terms.resize(&mut client, id, 80, 24);
        turn(&mut client, &mut b);
        b.terms.typed(&mut client, id, b"ho hi\r");
        turn(&mut client, &mut b);
        let stream = b.terms.get(id).unwrap().stream.unwrap();
        assert_eq!(hub.terminal_input(stream), b"echo hi\r");
        let lines = screen(&mut b, id);
        assert!(lines.contains(&"dev@demo:~/work$ echo hi".to_owned()), "{lines:?}");
        assert!(lines.contains(&"echo hi".to_owned()));
        // The credit given back covers what was read.
        let back: u64 = hub.notifications().iter().filter(|(m, _)| m == "stream/credit").skip(1).filter_map(|(_, p)| p["bytes"].as_u64()).sum();
        assert!(back > 0 || b.terms.get(id).unwrap().owed > 0);
    }

    #[test]
    fn exit_shows_the_code_and_restart_opens_a_new_shell() {
        let (mut client, hub, mut b, key) = rig();
        let id = b.new_panel_terminal(&client, &key, None).unwrap();
        b.terms.resize(&mut client, id, 80, 24);
        turn(&mut client, &mut b);
        b.terms.typed(&mut client, id, b"exit 3\r");
        turn(&mut client, &mut b);
        assert_eq!(b.terms.get(id).unwrap().state, SessionState::Exited(3));
        // Input to an ended shell goes nowhere.
        b.terms.typed(&mut client, id, b"ls\r");
        b.terms.restart(&mut client, id);
        turn(&mut client, &mut b);
        assert_eq!(opened(&hub).len(), 2);
        assert_eq!(b.terms.get(id).unwrap().state, SessionState::Running);
    }

    #[test]
    fn a_multi_line_paste_without_brackets_waits_for_the_user() {
        let (mut client, hub, mut b, key) = rig();
        let id = b.new_panel_terminal(&client, &key, None).unwrap();
        b.terms.resize(&mut client, id, 80, 24);
        turn(&mut client, &mut b);
        let stream = b.terms.get(id).unwrap().stream.unwrap();
        b.terms.paste(&mut client, id, "ls\nrm -rf build\n", false);
        assert!(b.terms.get(id).unwrap().paste_confirm.is_some());
        assert!(hub.terminal_input(stream).is_empty());
        b.terms.paste(&mut client, id, "ls\nrm -rf build\n", true);
        assert_eq!(hub.terminal_input(stream), b"ls\rrm -rf build\r");
    }

    #[test]
    fn input_beyond_the_credit_waits_for_more() {
        let (mut client, hub, mut b, key) = rig();
        let id = b.new_panel_terminal(&client, &key, None).unwrap();
        b.terms.resize(&mut client, id, 80, 24);
        turn(&mut client, &mut b);
        let stream = b.terms.get(id).unwrap().stream.unwrap();
        let big = vec![b'a'; 70_000];
        b.terms.input(&mut client, id, &big);
        assert_eq!(hub.terminal_input(stream).len(), 65_536, "the initial credit");
        b.terms.note(&mut client, "stream/credit", &json!({ "stream": stream, "bytes": 10_000 }));
        assert_eq!(hub.terminal_input(stream).len(), 70_000);
    }

    #[test]
    fn a_dropped_connection_reattaches_with_a_fresh_screen() {
        let (mut client, hub, mut b, key) = rig();
        let id = b.new_panel_terminal(&client, &key, None).unwrap();
        b.terms.resize(&mut client, id, 80, 24);
        turn(&mut client, &mut b);
        let set = |client: &mut Client, status| {
            let t = client.state.targets.iter_mut().find(|t| t.entry.id == key.target).unwrap();
            t.entry.state.status = status;
        };
        set(&mut client, yaac_protocol::attach::ConnectionStatus::Connecting);
        b.tick(&mut client, None, Instant::now());
        assert_eq!(b.terms.get(id).unwrap().state, SessionState::Reconnecting);
        set(&mut client, yaac_protocol::attach::ConnectionStatus::Connected);
        b.tick(&mut client, None, Instant::now());
        let attach = hub.requests().into_iter().find(|(_, m, _)| m == attach::names::TERMINAL_ATTACH).unwrap().2;
        assert_eq!(attach["terminal"], json!("demo-1"));
        turn(&mut client, &mut b);
        assert_eq!(b.terms.get(id).unwrap().state, SessionState::Running);
    }

    #[test]
    fn terminals_found_running_join_the_panel_and_attach_when_shown() {
        let (mut client, hub, mut b, key) = rig();
        let info = |n: &str| yaac_protocol::TerminalInfo {
            terminal: n.into(),
            cwd: "/work".into(),
            shell: "/usr/bin/zsh".into(),
            title: String::new(),
            thread_id: Some(key.thread.clone()),
            name: (n == "t2").then(|| "logs".to_owned()),
        };
        b.terms.adopt(&key.target, vec![info("t1"), info("t2")]);
        let p = b.terms.panel(&key).unwrap().clone();
        assert_eq!(p.tabs.len(), 2);
        assert!(!p.open, "found terminals do not pop the panel open");
        assert_eq!(b.terms.get(p.tabs[0]).unwrap().shell_name(), "zsh");
        assert_eq!(b.terms.get(p.tabs[1]).unwrap().title(), "logs", "a name kept on the target");
        b.tick(&mut client, None, Instant::now());
        assert!(!hub.requests().iter().any(|(_, m, _)| m == attach::names::TERMINAL_ATTACH), "nothing replays before it is shown");
        b.terms.mark_shown(p.tabs[0]);
        b.tick(&mut client, None, Instant::now());
        let attached: Vec<_> = hub.requests().into_iter().filter(|(_, m, _)| m == attach::names::TERMINAL_ATTACH).collect();
        assert_eq!(attached.len(), 1);
        assert_eq!(attached[0].2["terminal"], json!("t1"));
        // Listing again does not duplicate them.
        b.terms.adopt(&key.target, vec![info("t1")]);
        assert_eq!(b.terms.panel(&key).unwrap().tabs.len(), 2);
    }

    #[test]
    fn the_panel_toggles_cycles_and_closes_with_its_last_tab() {
        let (client, _hub, mut b, key) = rig();
        assert!(b.toggle_terminal_panel(&client, &key).unwrap(), "opening starts a terminal");
        let first = b.terms.panel(&key).unwrap().active;
        assert_eq!(b.terms.focus, Some(first));
        let second = b.new_panel_terminal(&client, &key, None).unwrap();
        b.terms.panel_cycle(&key, true);
        assert_eq!(b.terms.panel(&key).unwrap().active, first);
        b.terms.panel_cycle(&key, false);
        assert_eq!(b.terms.panel(&key).unwrap().active, second);
        assert!(!b.toggle_terminal_panel(&client, &key).unwrap());
        assert!(b.terms.focus_composer, "closing hands focus back to the composer");
        assert_eq!(b.terms.panel(&key).unwrap().tabs.len(), 2, "closing the panel keeps its terminals");
        b.toggle_terminal_panel(&client, &key).unwrap();
        b.terms.panel_maximise(&key);
        assert!(b.terms.panel(&key).unwrap().maximised);
        let mut c = client;
        b.terms.close(&mut c, first);
        b.terms.close(&mut c, second);
        assert!(!b.terms.panel_open(&key), "closing the last tab closes the panel");
    }

    #[test]
    fn opening_the_panel_over_a_full_screen_tab_shows_the_chat() {
        let (client, _hub, mut b, key) = rig();
        let tab = b.insert(&key);
        b.set_full_screen(&key, true);
        b.select(&key, tab);
        assert!(!b.chat_visible(&key));
        b.toggle_terminal_panel(&client, &key).unwrap();
        assert!(b.chat_visible(&key));
    }

    #[test]
    fn offline_targets_say_why() {
        let (mut client, _hub, mut b, key) = rig();
        let t = client.state.targets.iter_mut().find(|t| t.entry.id == key.target).unwrap();
        t.entry.state.daemon.as_mut().unwrap().terminal = false;
        t.entry.state.daemon.as_mut().unwrap().terminal_unavailable_reason = Some("node-pty is missing".into());
        assert_eq!(b.open_terminal(&client, &key, None).unwrap_err(), "node-pty is missing");
    }

    #[test]
    fn rename_in_place_names_the_terminal_on_the_target() {
        let (mut client, hub, mut b, key) = rig();
        let id = b.new_panel_terminal(&client, &key, None).unwrap();
        // Named before the shell exists: sent once it does.
        b.terms.begin_rename(id);
        assert_eq!(b.terms.renaming.as_ref().unwrap().text, "Terminal");
        b.terms.renaming.as_mut().unwrap().text = "  build watch ".into();
        b.terms.commit_rename(&mut client);
        assert_eq!(b.terms.get(id).unwrap().title(), "build watch");
        b.terms.resize(&mut client, id, 80, 24);
        turn(&mut client, &mut b);
        let renames = |hub: &DemoHub| hub.requests().into_iter().filter(|(_, m, _)| m == attach::names::TERMINAL_RENAME).map(|(_, _, p)| p).collect::<Vec<_>>();
        assert_eq!(renames(&hub).len(), 1);
        assert_eq!(renames(&hub)[0]["name"], json!("build watch"));
        // The shell's OSC title does not override the name.
        b.terms.get(id).unwrap().term.feed(b"\x1b]2;vim\x07");
        settle(&b, id);
        assert_eq!(b.terms.get(id).unwrap().title(), "build watch");
        // Esc cancels; unchanged text sends nothing.
        b.terms.begin_rename(id);
        b.terms.renaming.as_mut().unwrap().text = "other".into();
        b.terms.cancel_rename();
        b.terms.begin_rename(id);
        b.terms.commit_rename(&mut client);
        assert_eq!(renames(&hub).len(), 1);
        // Empty goes back to the automatic title.
        b.terms.begin_rename(id);
        b.terms.renaming.as_mut().unwrap().text = String::new();
        b.terms.commit_rename(&mut client);
        assert_eq!(b.terms.get(id).unwrap().title(), "vim");
        assert!(renames(&hub)[1].get("name").is_none());
        // Another window renamed it.
        let remote = b.terms.get(id).unwrap().remote.clone().unwrap();
        b.terms.note(&mut client, attach::names::TERMINAL_RENAMED, &json!({"target": key.target, "terminal": remote, "name": "api"}));
        assert_eq!(b.terms.get(id).unwrap().title(), "api");
    }
}
