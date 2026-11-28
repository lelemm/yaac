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
    /// A hub on another machine, as `host:port` (its Tailscale attach listener,
    /// docs/02-architecture.md#attaching-over-tailscale): the Android client's way in. Such a
    /// hub is never started, stopped or replaced from here; `paths` then only places this
    /// client's own files.
    pub remote: Option<String>,
    /// A hub that moved to one of this PC's targets, reached the way that target is
    /// (docs/15-hub-transfer.md#the-pcs-window): a command whose standard streams reach the
    /// hub (`wsl.exe … yaac hub-bridge`, `ssh … yaac hub-bridge`). Never started, stopped or
    /// replaced from here, like a remote hub, except that a hub in this PC's WSL has its keeper
    /// started when it does not run.
    pub bridge: Option<Bridge>,
}

/// A command whose standard streams reach a hub that moved to one of this PC's targets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bridge {
    /// `wsl.exe` or `ssh`.
    pub program: std::ffi::OsString,
    pub args: Vec<std::ffi::OsString>,
    /// Who it reaches, for messages (`Ubuntu (WSL)`, `me@box:22`).
    pub label: String,
    /// A hub in this PC's WSL: its keeper, started when it does not run.
    pub keeper: Option<yaac_hub::transfer::WslKeeper>,
}

impl Bridge {
    /// The bridge to a hub that moved to the target `via` (`wsl`, `ssh`), with this PC's hub
    /// files `local` (YAAC's own SSH key for that server and its confirmed host keys are kept
    /// there, invariant 12). `None` for any other transport.
    pub fn to(via: &yaac_protocol::attach::TargetTransport, local: &HubPaths) -> Option<Self> {
        use yaac_protocol::attach::TargetTransport;
        match via {
            TargetTransport::Wsl(w) => {
                let program = std::env::var_os("YAAC_WSL_EXE").filter(|v| !v.is_empty()).unwrap_or_else(|| "wsl.exe".into());
                let args = yaac_hub::transfer::wsl_bridge_argv(&w.distro, w.user.as_deref()).into_iter().map(Into::into).collect();
                let keeper = yaac_hub::transfer::WslKeeper { distro: w.distro.clone(), user: w.user.clone(), op: None };
                Some(Self { program, args, label: format!("{} (WSL)", w.distro), keeper: Some(keeper) })
            }
            TargetTransport::Ssh(t) => {
                let defaults = yaac_hub::ssh::SshDefaults::new(&local.data_dir, &local.runtime_dir, None, Vec::new());
                let args = defaults.command_args(t, &yaac_hub::transfer::ssh_bridge_remote());
                Some(Self { program: defaults.ssh_exe, args, label: yaac_hub::ssh::Ident::from_transport(t).label(), keeper: None })
            }
            _ => None,
        }
    }

    fn command(&self) -> Command {
        let mut c = Command::new(&self.program);
        c.args(&self.args);
        c
    }
}

impl AttachConfig {
    /// From the process environment: the hub's paths, this executable, the autostart entry.
    pub fn from_env() -> Result<Self, String> {
        let paths = HubPaths::from_env().map_err(|e| e.to_string())?;
        let exe = std::env::current_exe().map_err(|e| format!("cannot find the yaac binary: {e}"))?;
        let autostart = Autostart::from_env(exe.clone()).ok();
        Ok(Self { paths, exe, autostart, start_timeout: Duration::from_secs(8), remote: None, bridge: None })
    }

    /// A hub on another machine at `address` (`host:port`); `paths` holds this client's files.
    pub fn remote(address: String, paths: HubPaths) -> Self {
        Self { paths, exe: PathBuf::new(), autostart: None, start_timeout: Duration::from_secs(10), remote: Some(address), bridge: None }
    }

    /// A hub that moved to one of this PC's targets, through `bridge`; `paths` holds this
    /// client's files.
    pub fn bridged(bridge: Bridge, paths: HubPaths) -> Self {
        Self { paths, exe: PathBuf::new(), autostart: None, start_timeout: Duration::from_secs(25), remote: None, bridge: Some(bridge) }
    }

    /// Whether the hub is not this PC's local one (never started or replaced by this client).
    pub fn is_remote(&self) -> bool {
        self.remote.is_some() || self.bridge.is_some()
    }

    /// What a client of a hub on another machine says when the builds differ: it never
    /// replaces or restarts that hub (docs/14-android.md), so the hub's own advice (send
    /// `hub/shutdown`, update this window) does not apply. `None` for the local hub.
    pub fn mismatch_text(&self) -> Option<String> {
        if let Some(b) = &self.bridge {
            return Some(format!("The hub on {} is a different YAAC build. Move it back to this PC, or install the same build there.", b.label));
        }
        let address = self.remote.as_deref()?;
        Some(format!(
            "The hub on {} is a different YAAC build. Update YAAC on the PC or on this phone so they match.",
            machine_name(address)
        ))
    }
}

/// The machine an address names, for people: `desk.tailnet.ts.net:47380` → `desk`;
/// `[fd7a::1]:47380` or `100.64.0.7:47380` → the address without the port.
pub(crate) fn machine_name(address: &str) -> &str {
    let host = match address.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => host,
        _ => address,
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let is_ip = host.parse::<std::net::IpAddr>().is_ok();
    if is_ip { host } else { host.split('.').next().filter(|s| !s.is_empty()).unwrap_or(host) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_remote_hubs_build_mismatch_names_the_pc_and_never_suggests_replacing_it() {
        let paths = HubPaths { config_dir: "c".into(), data_dir: "d".into(), runtime_dir: "r".into() };
        let remote = AttachConfig::remote("desk.tailnet.ts.net:47380".into(), paths.clone());
        let text = remote.mismatch_text().unwrap();
        assert_eq!(text, "The hub on desk is a different YAAC build. Update YAAC on the PC or on this phone so they match.");
        assert!(!text.contains("shutdown"));
        assert_eq!(machine_name("100.64.0.7:47380"), "100.64.0.7");
        assert_eq!(machine_name("[fd7a:115c::1]:47380"), "fd7a:115c::1");
        assert_eq!(machine_name("desk"), "desk");
        let local = AttachConfig { paths, exe: PathBuf::new(), autostart: None, start_timeout: Duration::from_secs(1), remote: None, bridge: None };
        assert!(local.mismatch_text().is_none());
    }

    #[test]
    fn the_window_reaches_a_moved_hub_the_way_it_reached_its_target() {
        use yaac_protocol::attach::TargetTransport;
        let dir = tempfile::tempdir().unwrap();
        let paths = HubPaths::under(dir.path());
        let wsl: TargetTransport = serde_json::from_value(serde_json::json!({ "kind": "wsl", "distro": "Ubuntu", "user": "dev" })).unwrap();
        let b = Bridge::to(&wsl, &paths).unwrap();
        let args: Vec<String> = b.args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(args, yaac_hub::transfer::wsl_bridge_argv("Ubuntu", Some("dev")));
        assert_eq!(b.keeper.as_ref().map(|k| k.distro.as_str()), Some("Ubuntu"));
        let config = AttachConfig::bridged(b, paths.clone());
        assert!(config.is_remote(), "never started or replaced from here");
        assert!(config.mismatch_text().unwrap().contains("Ubuntu (WSL)"));

        // SSH: YAAC's own key for that server when it is here, its known hosts, never prompts.
        let ssh: TargetTransport = serde_json::from_value(serde_json::json!({ "kind": "ssh", "host": "box", "user": "me" })).unwrap();
        let text = |b: &Bridge| b.args.iter().map(|a| a.to_string_lossy().into_owned()).collect::<Vec<_>>();
        let plain = text(&Bridge::to(&ssh, &paths).unwrap());
        assert!(plain.contains(&"BatchMode=yes".to_owned()) && !plain.iter().any(|a| a.starts_with("IdentityFile=")));
        assert!(plain.iter().any(|a| a.contains("known_hosts")));
        assert_eq!(plain.last().unwrap(), &yaac_hub::transfer::ssh_bridge_remote());
        let TargetTransport::Ssh(t) = &ssh else { unreachable!() };
        let key = yaac_hub::ssh::SshDefaults::new(&paths.data_dir, &paths.runtime_dir, None, Vec::new()).key(&yaac_hub::ssh::Ident::from_transport(t));
        std::fs::create_dir_all(key.parent().unwrap()).unwrap();
        std::fs::write(&key, "k").unwrap();
        let keyed = text(&Bridge::to(&ssh, &paths).unwrap());
        assert!(keyed.iter().any(|a| a.starts_with("IdentityFile=")) && keyed.contains(&"IdentitiesOnly=yes".to_owned()));
        assert!(Bridge::to(&TargetTransport::Local(Default::default()), &paths).is_none());
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
    spawn_detached(&config.exe, &["hub".to_owned()], &config.paths)
}

/// Starts `exe args…` detached from the window, with the hub's profile (`YAAC_CONFIG_DIR`,
/// `YAAC_DATA_DIR`, `YAAC_RUNTIME_DIR`): `yaac hub`, or the WSL keeper `yaac wsl-hub`.
pub fn spawn_detached(exe: &std::path::Path, args: &[String], paths: &yaac_hub::HubPaths) -> std::io::Result<()> {
    let mut cmd = Command::new(exe);
    cmd.args(args)
        .env("YAAC_CONFIG_DIR", &paths.config_dir)
        .env("YAAC_DATA_DIR", &paths.data_dir)
        .env("YAAC_RUNTIME_DIR", &paths.runtime_dir)
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
    if let Some(bridge) = &config.bridge {
        return HubClient::connect_command(bridge.command(), move || w());
    }
    match &config.remote {
        Some(address) => connect_remote(address, waker),
        None => HubClient::connect_with_waker(&config.paths.socket(), move || w()),
    }
}

/// Resolves `host:port` (a Tailscale name or an address) and connects to the first address
/// that answers.
fn connect_remote(address: &str, waker: &Waker) -> std::io::Result<HubClient> {
    use std::net::ToSocketAddrs;
    let mut last = std::io::Error::new(std::io::ErrorKind::NotFound, format!("{address} has no address"));
    for addr in address.to_socket_addrs()? {
        let w = waker.clone();
        match HubClient::connect_tcp(addr, move || w()) {
            Ok(c) => return Ok(c),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// Connects to a running hub, or starts one and waits for it to listen.
fn connect_or_start(config: &AttachConfig, waker: &Waker, slot: &Mutex<Slot>) -> Result<HubClient, String> {
    if config.is_remote() {
        // A hub in this PC's WSL: its keeper keeps the distro and the hub running.
        if let Some(keeper) = config.bridge.as_ref().and_then(|b| b.keeper.as_ref())
            && !crate::wsl_keeper::running()
            && cfg!(windows)
            && let Err(e) = crate::wsl_keeper::start(keeper)
        {
            return Err(format!("could not start the hub in WSL: {e}"));
        }
        // Another machine's hub: connect or say why; it is never started from here.
        return connect(config, waker).map_err(|e| format!("could not reach the hub: {e}"));
    }
    if let Ok(c) = connect(config, waker) {
        return Ok(c);
    }
    // The hub moved back here, and `yaacd` is placing its files: they remove the tombstone
    // and start it. Never start a hub that has moved (it would refuse anyway).
    if config.paths.moved_file().is_file() {
        let deadline = Instant::now() + config.start_timeout * 3;
        while config.paths.moved_file().is_file() {
            if Instant::now() >= deadline {
                return Err("the hub moved away from this PC".into());
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        let deadline = Instant::now() + config.start_timeout;
        loop {
            match connect(config, waker) {
                Ok(c) => return Ok(c),
                Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
                Err(_) => break,
            }
        }
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
            if after_shutdown && (config.is_remote() || !wait_for_lock(&config, config.start_timeout)) {
                if let Ok(mut g) = s.lock() {
                    g.error = Some("the old hub did not exit".into());
                }
                waker();
                return;
            }
            let outcome = connect_or_start(&config, &waker, &s);
            // The window reads the cache file directly (read-only); the hub is its only writer.
            // A remote hub's cache is on its own machine: nothing is read here.
            let prefs = if config.is_remote() { Vec::new() } else { read_prefs(&config) };
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
