//! Attaching the window to the hub (docs/02-architecture.md#processes).
//!
//! The window owns no target connection: it connects to the hub's user-only socket, and
//! everything goes through the attach protocol. [`HubAttach`] is the window's
//! [`HubLink`]: it connects (or starts `yaac hub` detached and waits for it to listen) on a
//! background thread, so the UI thread never blocks; messages sent before the connection is
//! up are queued; every arriving message calls the waker, which requests a repaint — there is
//! no polling timer.
//!
//! `hub/hello` has three outcomes, read from [`yaac_core::state::HelloOutcome`]:
//!
//! | Outcome | What the window does |
//! | --- | --- |
//! | same build | attached |
//! | window newer | `hub/shutdown` to the old hub, wait for its lock to be released, start this build's hub, attach again ([`HubAttach::restart`]) |
//! | window older | show "update YAAC" and attach to nothing |

use std::collections::VecDeque;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use yaac_core::link::HubLink;
use yaac_hub::HubPaths;
use yaac_hub::autostart::{Autostart, install_autostart};
use yaac_hub::client::HubClient;
use yaac_protocol::rpc::Message;

/// Called from a background thread whenever something arrives (the window requests a repaint).
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// Where the hub is and how to start it.
#[derive(Clone, Debug)]
pub struct AttachConfig {
    /// The hub's files (socket, lock, settings).
    pub paths: HubPaths,
    /// The `yaac` binary to start as `yaac hub`.
    pub exe: PathBuf,
    /// Where the login entry goes; `None` never installs one.
    pub autostart: Option<Autostart>,
    /// How long to wait for a started hub to listen.
    pub start_timeout: Duration,
}

impl AttachConfig {
    /// From the process environment: the hub's paths, this executable, the autostart entry.
    pub fn from_env() -> Result<Self, String> {
        let paths = HubPaths::from_env().map_err(|e| e.to_string())?;
        let exe = std::env::current_exe().map_err(|e| format!("cannot find the yaac binary: {e}"))?;
        let autostart = Autostart::from_env(exe.clone()).ok();
        Ok(Self { paths, exe, autostart, start_timeout: Duration::from_secs(8) })
    }
}

/// The connection's state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkStatus {
    /// Connecting, or starting the hub.
    Connecting,
    /// Connected.
    Connected,
    /// The hub could not be reached or started; the message says why.
    Failed(String),
    /// The connection closed (the hub exited).
    Closed,
}

#[derive(Default)]
struct Slot {
    client: Option<HubClient>,
    error: Option<String>,
    /// This attach found no settings file: a first run. The login entry was installed.
    first_run: bool,
    /// The window-side choices read from the cache file's `ui_state` once connected.
    prefs: Option<Vec<(String, serde_json::Value)>>,
}

/// The window's link to the hub.
pub struct HubAttach {
    slot: Arc<Mutex<Slot>>,
    outbox: VecDeque<Message>,
}

impl std::fmt::Debug for HubAttach {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HubAttach").field("status", &self.status()).finish_non_exhaustive()
    }
}

/// Starts `yaac hub` detached: its own process group, no terminal, the resolved paths passed
/// explicitly so it uses exactly the files the window looks at.
pub fn spawn_hub(config: &AttachConfig) -> std::io::Result<()> {
    let mut cmd = Command::new(&config.exe);
    cmd.arg("hub")
        .env("YAAC_CONFIG_DIR", &config.paths.config_dir)
        .env("YAAC_DATA_DIR", &config.paths.data_dir)
        .env("YAAC_RUNTIME_DIR", &config.paths.runtime_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    let mut child = {
        // No console, its own process group (a Ctrl+C in the window's terminal does not reach
        // it), and out of the window's job when the job allows it, so closing whatever started
        // the window does not take the hub down with it.
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        let flags = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
        match cmd.creation_flags(flags | CREATE_BREAKAWAY_FROM_JOB).spawn() {
            Ok(child) => child,
            // The job does not allow breaking away.
            Err(_) => cmd.creation_flags(flags).spawn()?,
        }
    };
    #[cfg(not(windows))]
    let mut child = cmd.spawn()?;
    // Reap it if it exits while the window still runs (the hub normally outlives the window).
    std::thread::Builder::new().name("yaac-hub-reaper".into()).spawn(move || {
        let _ = child.wait();
    })?;
    Ok(())
}

fn connect(config: &AttachConfig, waker: &Waker) -> std::io::Result<HubClient> {
    let w = waker.clone();
    HubClient::connect_with_waker(&config.paths.socket(), move || w())
}

/// Connects to a running hub, or starts one and waits for it to listen.
fn connect_or_start(config: &AttachConfig, waker: &Waker, slot: &Mutex<Slot>) -> Result<HubClient, String> {
    if let Ok(c) = connect(config, waker) {
        return Ok(c);
    }
    // No hub is listening. A missing settings file means this is the first run: the owner
    // decided the hub starts at login, so the entry is installed now (Settings can turn it off).
    if !config.paths.settings_file().exists() {
        if let Some(a) = &config.autostart {
            let _ = install_autostart(a);
        }
        if let Ok(mut s) = slot.lock() {
            s.first_run = true;
        }
    }
    spawn_hub(config).map_err(|e| format!("could not start the hub: {e}"))?;
    let deadline = Instant::now() + config.start_timeout;
    loop {
        match connect(config, waker) {
            Ok(c) => return Ok(c),
            Err(e) if Instant::now() >= deadline => return Err(format!("the hub did not start: {e}")),
            Err(_) => std::thread::sleep(Duration::from_millis(25)),
        }
    }
}

/// Waits until no hub holds the lock (the old one has exited), up to `timeout`.
fn wait_for_lock(config: &AttachConfig, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(lock) = yaac_hub::HubLock::acquire(&config.paths.lock_file()) {
            drop(lock);
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn read_prefs(config: &AttachConfig) -> Vec<(String, serde_json::Value)> {
    let Ok(cache) = yaac_hub::cache::Cache::open_read_only(&config.paths.cache_file()) else { return Vec::new() };
    yaac_core::prefs::keys::ALL
        .iter()
        .filter_map(|k| cache.ui_state(k).ok().flatten().map(|v| ((*k).to_owned(), v)))
        .collect()
}

impl HubAttach {
    /// Connect (or start the hub) in the background.
    pub fn start(config: AttachConfig, waker: Waker) -> Self {
        Self::launch(config, waker, false)
    }

    /// After the old hub answered `hub/shutdown`: wait for it to let go of its lock, start this
    /// build's hub, and connect to it.
    pub fn restart(config: AttachConfig, waker: Waker) -> Self {
        Self::launch(config, waker, true)
    }

    fn launch(config: AttachConfig, waker: Waker, after_shutdown: bool) -> Self {
        let slot: Arc<Mutex<Slot>> = Arc::default();
        let s = slot.clone();
        let spawned = std::thread::Builder::new().name("yaac-attach".into()).spawn(move || {
            if after_shutdown && !wait_for_lock(&config, config.start_timeout) {
                if let Ok(mut g) = s.lock() {
                    g.error = Some("the old hub did not exit".into());
                }
                waker();
                return;
            }
            let outcome = connect_or_start(&config, &waker, &s);
            // The window reads the cache file directly (read-only); the hub is its only writer.
            let prefs = read_prefs(&config);
            if let Ok(mut g) = s.lock() {
                g.prefs = Some(prefs);
                match outcome {
                    Ok(c) => g.client = Some(c),
                    Err(e) => g.error = Some(e),
                }
            }
            waker();
        });
        if let Err(e) = spawned
            && let Ok(mut g) = slot.lock()
        {
            g.error = Some(format!("could not start a thread: {e}"));
        }
        Self { slot, outbox: VecDeque::new() }
    }

    /// Where the connection is.
    pub fn status(&self) -> LinkStatus {
        let Ok(g) = self.slot.lock() else { return LinkStatus::Failed("internal error".into()) };
        match (&g.client, &g.error) {
            (Some(c), _) if c.is_connected() => LinkStatus::Connected,
            (Some(_), _) => LinkStatus::Closed,
            (None, Some(e)) => LinkStatus::Failed(e.clone()),
            (None, None) => LinkStatus::Connecting,
        }
    }

    /// Whether this attach found a first run (and installed the login entry).
    pub fn first_run(&self) -> bool {
        self.slot.lock().is_ok_and(|g| g.first_run)
    }

    /// The persisted window-side choices, once (after the connection is up).
    pub fn take_prefs(&mut self) -> Option<Vec<(String, serde_json::Value)>> {
        self.slot.lock().ok()?.prefs.take()
    }

    fn flush(&mut self) {
        let Ok(g) = self.slot.lock() else { return };
        let Some(c) = &g.client else { return };
        while let Some(m) = self.outbox.pop_front() {
            if c.send(m).is_err() {
                break;
            }
        }
    }
}

impl HubLink for HubAttach {
    fn send(&mut self, message: Message) {
        self.outbox.push_back(message);
        self.flush();
    }

    fn poll(&mut self) -> Option<Message> {
        self.flush();
        let g = self.slot.lock().ok()?;
        g.client.as_ref()?.try_recv()
    }

    fn send_data(&mut self, stream: yaac_protocol::StreamId, bytes: Vec<u8>) {
        // Ordered after any control message already queued for the hub.
        self.flush();
        if let Ok(g) = self.slot.lock()
            && let Some(c) = &g.client
        {
            let _ = c.send_data(stream, bytes);
        }
    }

    fn poll_data(&mut self) -> Option<(yaac_protocol::StreamId, Vec<u8>)> {
        let g = self.slot.lock().ok()?;
        g.client.as_ref()?.try_recv_data()
    }
}

/// A [`HubLink`] that shares one [`HubAttach`] with the window, which also needs to see the
/// connection's status.
#[derive(Clone, Debug)]
pub struct SharedAttach(pub Arc<Mutex<HubAttach>>);

impl HubLink for SharedAttach {
    fn send(&mut self, message: Message) {
        if let Ok(mut a) = self.0.lock() {
            a.send(message);
        }
    }

    fn poll(&mut self) -> Option<Message> {
        self.0.lock().ok()?.poll()
    }

    fn send_data(&mut self, stream: yaac_protocol::StreamId, bytes: Vec<u8>) {
        if let Ok(mut a) = self.0.lock() {
            a.send_data(stream, bytes);
        }
    }

    fn poll_data(&mut self) -> Option<(yaac_protocol::StreamId, Vec<u8>)> {
        self.0.lock().ok()?.poll_data()
    }
}
