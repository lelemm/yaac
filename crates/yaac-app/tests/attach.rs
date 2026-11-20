//! The window attaches to a real hub without opening a window: `yaac hub` runs headless in a
//! temporary profile (the `YAAC_CONFIG_DIR` / `YAAC_DATA_DIR` / `YAAC_RUNTIME_DIR`
//! overrides), and the window's own link completes `hub/hello` — in all three outcomes.
//!
//! Runs on Unix (a socket file) and on Windows (a named pipe; the login entry goes to a
//! throwaway registry key that is deleted afterwards).
#![cfg(all(any(unix, windows), feature = "hub"))]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use yaac_app::attach::{AttachConfig, HubAttach, LinkStatus, SharedAttach};
use yaac_core::client::Client;
use yaac_core::state::HelloOutcome;
use yaac_hub::HubPaths;
use yaac_hub::autostart::{Autostart, autostart_installed};
use yaac_protocol::ClientId;

/// The `yaac` binary: `YAAC_TEST_EXE` when the tests run somewhere other than where they
/// were built (a cross-compiled test run on Windows), else the one Cargo built.
fn exe() -> PathBuf {
    std::env::var_os("YAAC_TEST_EXE").map_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_yaac")), PathBuf::from)
}
const WAIT: Duration = Duration::from_secs(20);

struct Profile {
    dir: tempfile::TempDir,
}

impl Profile {
    fn new() -> Self {
        // Under /tmp, so the socket path stays short.
        #[cfg(unix)]
        let dir = tempfile::Builder::new().prefix("ya").tempdir_in("/tmp").expect("temp dir");
        #[cfg(windows)]
        let dir = tempfile::Builder::new().prefix("ya").tempdir().expect("temp dir");
        Self { dir }
    }

    /// Where this profile's login entry goes: a private directory, or on Windows a throwaway
    /// key under `HKEY_CURRENT_USER\Software`.
    fn autostart(&self) -> Autostart {
        #[cfg(unix)]
        let at = self.var("autostart");
        #[cfg(windows)]
        let at = PathBuf::from(self.registry_key());
        Autostart::new(at, exe())
    }

    #[cfg(windows)]
    fn registry_key(&self) -> String {
        format!(r"Software\YAAC-test-{}", self.dir.path().file_name().unwrap().to_string_lossy())
    }

    fn var(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    /// The paths the window resolves from the three overrides.
    fn paths(&self) -> HubPaths {
        let (c, d, r) = (self.var("config"), self.var("data"), self.var("run"));
        HubPaths::from_lookup(move |k| match k {
            "YAAC_CONFIG_DIR" => Some(c.clone().into_os_string()),
            "YAAC_DATA_DIR" => Some(d.clone().into_os_string()),
            "YAAC_RUNTIME_DIR" => Some(r.clone().into_os_string()),
            _ => None,
        })
        .expect("paths")
    }

    fn config(&self) -> AttachConfig {
        AttachConfig {
            paths: self.paths(),
            exe: exe(),
            autostart: Some(self.autostart()),
            start_timeout: Duration::from_secs(15),
        }
    }

    /// A settings file with no targets, so the hub starts no daemon.
    fn no_targets(&self) {
        std::fs::create_dir_all(self.var("config")).unwrap();
        std::fs::write(self.var("config").join("hub.json"), r#"{"targets":[],"hub":{"always":false,"idleExitSecs":300}}"#).unwrap();
    }

    /// `yaac hub`, headless, in this profile.
    fn spawn_hub(&self) -> Child {
        Command::new(exe())
            .arg("hub")
            .env("YAAC_CONFIG_DIR", self.var("config"))
            .env("YAAC_DATA_DIR", self.var("data"))
            .env("YAAC_RUNTIME_DIR", self.var("run"))
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("yaac hub")
    }
}

#[cfg(windows)]
impl Drop for Profile {
    fn drop(&mut self) {
        let _ = yaac_winsys::registry_delete_tree(&self.registry_key());
    }
}

/// Whether a hub listens. A named pipe cannot be probed without connecting to it.
fn listening(socket: &Path) -> bool {
    #[cfg(unix)]
    return socket.exists();
    #[cfg(windows)]
    return yaac_hub::client::HubClient::connect(socket).is_ok();
}

fn attach(config: AttachConfig) -> (Client, Arc<Mutex<HubAttach>>) {
    let a = Arc::new(Mutex::new(HubAttach::start(config, Arc::new(|| {}))));
    let c = Client::new(Box::new(SharedAttach(a.clone())), yaac_app::BUILD, ClientId::new("test-window"));
    (c, a)
}

fn pump_until(c: &mut Client, what: &str, done: impl Fn(&Client) -> bool) {
    let deadline = Instant::now() + WAIT;
    while !done(c) {
        assert!(Instant::now() < deadline, "timed out waiting for {what}: {:?} / {:?}", c.state.hub, c.errors);
        c.pump();
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn wait_exit(child: &mut Child) {
    let deadline = Instant::now() + WAIT;
    loop {
        if child.try_wait().expect("wait").is_some() {
            return;
        }
        assert!(Instant::now() < deadline, "the hub did not exit");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn wait_gone(socket: &Path) {
    let deadline = Instant::now() + WAIT;
    while listening(socket) {
        assert!(Instant::now() < deadline, "the hub socket is still there");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn yaac_hub_runs_headless_and_the_window_attaches() {
    let p = Profile::new();
    p.no_targets();
    let mut hub = p.spawn_hub();
    let (mut c, a) = attach(p.config());
    pump_until(&mut c, "hello", |c| c.state.hub.outcome != HelloOutcome::Pending);
    assert_eq!(c.state.hub.outcome, HelloOutcome::Attached);
    assert!(c.state.hub.attached);
    assert_eq!(c.state.hub.build.as_deref(), Some(yaac_app::BUILD));
    assert_eq!(a.lock().unwrap().status(), LinkStatus::Connected);
    // The hub's settings arrive after hello (the autostart switch reads them).
    pump_until(&mut c, "hub settings", |c| c.state.hub.always.is_some());
    assert_eq!(c.state.hub.always, Some(false));
    // A running hub with settings is not a first run: no login entry is installed.
    assert!(!autostart_installed(&p.autostart()));
    c.shutdown_hub();
    pump_until(&mut c, "shutdown", |c| c.state.hub.shutdown_answered);
    wait_exit(&mut hub);
}

#[test]
fn the_window_starts_the_hub_when_none_runs_and_installs_autostart_on_first_run() {
    let p = Profile::new();
    let (mut c, a) = attach(p.config());
    pump_until(&mut c, "hello", |c| c.state.hub.outcome != HelloOutcome::Pending);
    assert_eq!(c.state.hub.outcome, HelloOutcome::Attached);
    assert!(a.lock().unwrap().first_run());
    assert!(autostart_installed(&p.autostart()), "first run installs the login entry");
    assert!(p.var("config").join("hub.json").exists());
    c.shutdown_hub();
    pump_until(&mut c, "shutdown", |c| c.state.hub.shutdown_answered);
    wait_gone(&p.paths().socket());
}

fn in_process_hub(p: &Profile, build: &str) -> std::thread::JoinHandle<()> {
    p.no_targets();
    let local = yaac_transport::LocalConfig::from_lookup(Path::new("."), |_| None);
    let mut config = yaac_hub::HubConfig::new(build, p.paths(), Arc::new(yaac_hub::DefaultConnectors { local }));
    config.seed_local_target = false;
    config.handle_signals = false;
    config.idle_exit = Some(Duration::from_secs(600));
    let socket = p.paths().socket();
    let h = std::thread::spawn(move || {
        let _ = yaac_hub::run_hub(config);
    });
    let deadline = Instant::now() + WAIT;
    while !listening(&socket) {
        assert!(Instant::now() < deadline, "the in-process hub did not listen");
        std::thread::sleep(Duration::from_millis(10));
    }
    h
}

#[test]
fn a_newer_window_replaces_an_older_hub() {
    let p = Profile::new();
    let old = in_process_hub(&p, "0.0.1");
    let (mut c, _a) = attach(p.config());
    pump_until(&mut c, "hello", |c| c.state.hub.outcome != HelloOutcome::Pending);
    assert_eq!(c.state.hub.outcome, HelloOutcome::HubOutdated { hub_build: "0.0.1".into() });
    assert!(!c.state.hub.attached);
    // What the window does: ask the old hub to stop, then start this build's hub.
    c.shutdown_hub();
    pump_until(&mut c, "shutdown", |c| c.state.hub.shutdown_answered);
    old.join().expect("the old hub exited");
    let a = Arc::new(Mutex::new(HubAttach::restart(p.config(), Arc::new(|| {}))));
    let mut c = Client::new(Box::new(SharedAttach(a)), yaac_app::BUILD, ClientId::new("test-window"));
    pump_until(&mut c, "hello to the new hub", |c| c.state.hub.outcome != HelloOutcome::Pending);
    assert_eq!(c.state.hub.outcome, HelloOutcome::Attached);
    assert_eq!(c.state.hub.build.as_deref(), Some(yaac_app::BUILD));
    c.shutdown_hub();
    pump_until(&mut c, "shutdown", |c| c.state.hub.shutdown_answered);
    wait_gone(&p.paths().socket());
}

#[test]
fn an_older_window_is_told_to_update_and_attaches_to_nothing() {
    let p = Profile::new();
    let hub = in_process_hub(&p, "999.0.0");
    let (mut c, _a) = attach(p.config());
    pump_until(&mut c, "hello", |c| c.state.hub.outcome != HelloOutcome::Pending);
    assert!(matches!(&c.state.hub.outcome, HelloOutcome::WindowOutdated { message } if message.contains("Update YAAC")), "{:?}", c.state.hub.outcome);
    assert!(!c.state.hub.attached);
    // Stop the hub as a window of its own build would.
    let same = yaac_hub::client::HubClient::connect(&p.paths().socket()).expect("connect");
    same.request_blocking("hub/hello", serde_json::json!({ "build": "999.0.0" }), WAIT).expect("hello");
    let _ = same.request_blocking("hub/shutdown", serde_json::json!({}), WAIT);
    hub.join().expect("the hub exited");
}
