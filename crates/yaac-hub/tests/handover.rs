//! Updating an older `yaacd` (docs/02-architecture.md#one-serving-daemon-per-user): the hub
//! asks an older serving daemon to hand over when idle, or at once with **Update now**, and
//! reconnects through its one normal connection path to the newer build.
//!
//! Runs the **real** daemon bundle (`daemon/dist/yaacd.mjs`) on its fake provider, in two
//! builds made from it: `0.1.0+41` and `0.1.0+42` (the same code, its build string replaced).
//! Skipped, not failed, when the bundle or `node` is missing.

mod support;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use support::{BUILD, Client, Home, RunningHub, wait_until};
use yaac_hub::protocol::names as ext;
use yaac_hub::settings::TargetDef;
use yaac_hub::{ConnectorFactory, HubConfig};
use yaac_protocol::TargetId;
use yaac_protocol::attach::{self, ConnectionStatus, DaemonUpdatePhase, TargetEntry};
use yaac_transport::{Connector, LocalConfig, LocalConnector};

const OLD: &str = "0.1.0+41";
const NEW: &str = "0.1.0+42";

/// The local connector, swappable (a test changes which build it says it starts).
struct Swappable(Mutex<LocalConfig>);

impl ConnectorFactory for Swappable {
    fn connector(&self, _target: &TargetDef) -> Result<Arc<dyn Connector>, String> {
        Ok(Arc::new(LocalConnector::new(self.0.lock().unwrap().clone())))
    }
}

struct Rig {
    home: Home,
    runtime: OsString,
    daemon_home: PathBuf,
    socket: PathBuf,
    old: PathBuf,
    new: PathBuf,
    factory: Arc<Swappable>,
    /// The older daemon, started before the hub.
    serving: Option<Child>,
}

impl Rig {
    fn new() -> Option<Self> {
        let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../daemon/dist/yaacd.mjs");
        if !source.is_file() {
            eprintln!("SKIPPED: daemon/dist/yaacd.mjs does not exist (build the daemon to run this test)");
            return None;
        }
        let runtime = std::env::var_os("YAAC_RUNTIME").unwrap_or_else(|| "node".into());
        let Ok(out) = Command::new(&runtime).arg(&source).arg("version").output() else {
            eprintln!("SKIPPED: the runtime {runtime:?} does not run");
            return None;
        };
        let built = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        let bytes = std::fs::read(&source).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        let quoted = format!("\"{built}\"");
        assert!(text.contains(&quoted), "ASSUMPTION: the bundle carries its build {quoted} as a string");
        let home = Home::new();
        let variant = |build: &str| -> PathBuf {
            let dir = home.dir.path().join("daemon").join(build);
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("yaacd.mjs");
            std::fs::write(&path, text.replace(&quoted, &format!("\"{build}\""))).unwrap();
            path
        };
        let (old, new) = (variant(OLD), variant(NEW));
        let daemon_home = home.dir.path().join("target-home");
        std::fs::create_dir_all(&daemon_home).unwrap();
        let socket = home.dir.path().join("d.sock");
        let factory = Arc::new(Swappable(Mutex::new(LocalConfig::from_lookup(Path::new("."), |_| None))));
        let rig = Self { home, runtime, daemon_home, socket, old, new, factory, serving: None };
        rig.use_bundle(&rig.new.clone(), Some(NEW));
        Some(rig)
    }

    fn env(&self) -> Vec<(&'static str, OsString)> {
        vec![
            ("YAAC_FAKE_PROVIDER", "1".into()),
            // Never let this test probe or start the user's real harnesses.
            ("YAAC_CODEX_PROVIDER", "0".into()),
            ("YAAC_CLAUDE_PROVIDER", "0".into()),
            ("YAAC_HOME", self.daemon_home.join(".yaac").into()),
            ("YAAC_SOCKET", self.socket.clone().into()),
            ("YAAC_IDLE_TIMEOUT_MS", "3000".into()),
            ("HOME", self.daemon_home.clone().into()),
        ]
    }

    /// The hub's local connector runs `bundle` and says it starts `build`.
    fn use_bundle(&self, bundle: &Path, build: Option<&str>) {
        let mut config = LocalConfig { runtime: self.runtime.clone(), bundle: bundle.to_owned(), env: vec![], inherit_stderr: false, build: build.map(str::to_owned), install_files: vec![] };
        for (k, v) in self.env() {
            config = config.with_env(k, v);
        }
        *self.factory.0.lock().unwrap() = config;
    }

    /// Starts `bundle serve` before any hub, as if an older YAAC had left it running.
    fn serve(&mut self, bundle: &Path) {
        let mut c = Command::new(&self.runtime);
        c.arg(bundle).arg("serve").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        for (k, v) in self.env() {
            c.env(k, v);
        }
        // It must not idle out before the hub attaches, however loaded the machine is: only a
        // hand-over may end it (the test kills it otherwise).
        c.env("YAAC_IDLE_TIMEOUT_MS", "0");
        self.serving = Some(c.spawn().expect("start the older daemon"));
        wait_until("the older daemon listening", || self.socket.exists());
    }

    /// Whether the daemon started by [`Rig::serve`] is still running.
    fn serving_alive(&mut self) -> bool {
        self.serving.as_mut().is_some_and(|c| c.try_wait().unwrap().is_none())
    }

    fn hub(&self) -> (RunningHub, Client, TargetId) {
        let mut config = HubConfig::new(BUILD, self.home.paths(), self.factory.clone());
        config.seed_local_target = false;
        config.test_methods = true;
        config.handle_signals = false;
        config.idle_exit = Some(Duration::from_secs(600));
        config.backoff = yaac_hub::Backoff { initial: Duration::from_millis(50), max: Duration::from_millis(500) };
        let hub = self.home.start_with(config);
        let c = hub.client();
        let target = c.add_target("local").id;
        (hub, c, target)
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        if let Some(mut c) = self.serving.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

fn entry(c: &Client, target: &TargetId) -> TargetEntry {
    c.targets().into_iter().find(|t| &t.id == target).expect("the target")
}

/// Waits until `target` is connected to the daemon of `build`.
fn wait_serving(c: &Client, target: &TargetId, build: &str) -> TargetEntry {
    let deadline = Instant::now() + Duration::from_secs(40);
    loop {
        let t = entry(c, target);
        if t.state.status == ConnectionStatus::Connected && t.state.daemon.as_ref().is_some_and(|d| d.build == build) {
            return t;
        }
        assert!(Instant::now() < deadline, "yaacd {build} never served: {:?}", t.state);
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn wait_phase(c: &Client, target: &TargetId, phase: &DaemonUpdatePhase) -> TargetEntry {
    let deadline = Instant::now() + Duration::from_secs(40);
    loop {
        let t = entry(c, target);
        if t.state.update.as_ref().is_some_and(|u| &u.phase == phase) {
            return t;
        }
        assert!(Instant::now() < deadline, "the update never reached {phase:?}: {:?}", t.state);
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The hub's connection steps for a target, as `step: detail` lines.
fn steps(c: &Client, target: &TargetId) -> Vec<String> {
    let r = c.call(ext::TARGETS_LOG, json!({ "id": target })).expect("log");
    r["steps"].as_array().unwrap().iter().map(|s| format!("{}: {}", s["step"].as_str().unwrap_or(""), s["detail"].as_str().unwrap_or(""))).collect()
}

fn asks(c: &Client, target: &TargetId) -> usize {
    steps(c, target).iter().filter(|s| s.contains("asked yaacd")).count()
}

fn instance(c: &Client, target: &TargetId) -> String {
    let providers = c.routed(target, "provider/list", json!({})).expect("provider/list");
    providers["providers"][0]["instance"].as_str().expect("ASSUMPTION: YAAC_FAKE_PROVIDER=1 lists an instance").to_owned()
}

fn start_thread(c: &Client, rig: &Rig, target: &TargetId, text: &str) -> String {
    let instance = instance(c, target);
    let started = c
        .routed(target, "thread/start", json!({
            "opId": format!("op-{}", text.len()),
            "instance": instance,
            "cwd": rig.daemon_home.display().to_string(),
            "input": [{ "type": "text", "text": text }]
        }))
        .expect("thread/start");
    started["threadId"].as_str().expect("threadId").to_owned()
}

fn items(c: &Client, target: &TargetId, thread: &str) -> Vec<Value> {
    let read = c.routed(target, "thread/read", json!({ "threadId": thread })).expect("thread/read");
    read["items"].as_array().cloned().unwrap_or_default()
}

fn running(c: &Client, target: &TargetId, thread: &str) -> bool {
    let r = c.routed(target, "thread/lookup", json!({ "id": thread }));
    r.ok().is_some_and(|v| v["threads"].as_array().is_some_and(|t| t.iter().any(|s| s["state"] == "running")))
}

fn update_now(c: &Client, target: &TargetId) -> Result<Value, yaac_protocol::rpc::ErrorObject> {
    c.call(attach::names::TARGETS_UPDATE_DAEMON, json!({ "id": target }))
}

/// Restarts the target's connection, so the connector is asked for again.
fn reconnect_target(c: &Client, target: &TargetId) {
    c.call(attach::names::TARGETS_ENABLE, json!({ "id": target, "enabled": false })).unwrap();
    c.wait_status(target, ConnectionStatus::Disabled);
    c.call(attach::names::TARGETS_ENABLE, json!({ "id": target, "enabled": true })).unwrap();
}

/// The hand-over tests each start real daemons and Node processes and wait on them; run in
/// parallel on a slow CI runner they starve each other past [`support::WAIT`]. One at a time.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

#[test]
fn an_older_daemon_hands_over_at_idle_and_the_newest_build_serves() {
    let _serial = serial();
    let Some(mut rig) = Rig::new() else { return };
    rig.serve(&rig.old.clone());
    let (hub, c, target) = rig.hub();
    let t = wait_serving(&c, &target, NEW);
    assert!(t.state.update.is_none(), "the update is cleared once the newer build serves: {:?}", t.state);
    let d = t.state.daemon.unwrap();
    assert!(!d.older && !d.can_update, "{d:?}");
    wait_until("the older daemon exited", || !rig.serving_alive());
    let log = steps(&c, &target);
    assert!(log.iter().any(|s| s.contains(&format!("asked yaacd {OLD} to hand over when idle, to {NEW}"))), "{log:#?}");
    assert!(log.iter().any(|s| s.starts_with("updated:") && s.contains(&format!("{OLD} -> {NEW}"))), "{log:#?}");
    assert_eq!(asks(&c, &target), 1);
    // Update now has nothing to do any more.
    let e = update_now(&c, &target).unwrap_err();
    assert_eq!(e.code.as_str(), "update_unavailable", "{e:?}");
    drop(c);
    hub.shutdown();
}

#[test]
fn no_hand_over_unless_the_hub_holds_a_newer_build() {
    let _serial = serial();
    // The same build serving.
    let Some(mut rig) = Rig::new() else { return };
    rig.serve(&rig.new.clone());
    let (hub, c, target) = rig.hub();
    let t = wait_serving(&c, &target, NEW);
    std::thread::sleep(Duration::from_millis(1500));
    assert!(rig.serving_alive(), "the daemon was left alone");
    assert_eq!(asks(&c, &target), 0);
    let d = t.state.daemon.clone().unwrap();
    assert!(!d.can_update && !d.older && t.state.update.is_none(), "{:?}", t.state);
    assert_eq!(update_now(&c, &target).unwrap_err().code.as_str(), "update_unavailable");
    drop(c);
    hub.shutdown();
    drop(rig);

    // An older build serving, but a hub that does not know what its connector starts (no
    // bundle build: a development hub, or none embedded).
    let Some(mut rig) = Rig::new() else { return };
    rig.serve(&rig.old.clone());
    rig.use_bundle(&rig.new.clone(), None);
    let (hub, c, target) = rig.hub();
    let t = wait_serving(&c, &target, OLD);
    std::thread::sleep(Duration::from_millis(1500));
    assert!(rig.serving_alive(), "the daemon was left alone");
    assert_eq!(asks(&c, &target), 0);
    assert!(!t.state.daemon.unwrap().can_update);
    assert_eq!(update_now(&c, &target).unwrap_err().code.as_str(), "update_unavailable");
    drop(c);
    hub.shutdown();
}

#[test]
fn a_hand_over_that_brings_back_the_same_build_is_not_asked_again() {
    let _serial = serial();
    let Some(mut rig) = Rig::new() else { return };
    rig.serve(&rig.old.clone());
    // The connector claims build 42 but runs the old bundle: after the hand-over, 41 serves again.
    rig.use_bundle(&rig.old.clone(), Some(NEW));
    let (hub, c, target) = rig.hub();
    let t = wait_phase(&c, &target, &DaemonUpdatePhase::Stuck);
    wait_until("the first daemon exited", || !rig.serving_alive());
    let u = t.state.update.unwrap();
    assert_eq!((u.from.as_str(), u.to.as_str()), (OLD, NEW));
    assert!(u.detail.as_deref().is_some_and(|d| d.contains("does not ask again")), "{u:?}");
    let t = wait_serving(&c, &target, OLD);
    // Left alone: connected, stuck, asked once.
    std::thread::sleep(Duration::from_millis(2000));
    let t2 = entry(&c, &target);
    assert_eq!(t2.state.status, ConnectionStatus::Connected);
    assert_eq!(t2.state.update.as_ref().map(|u| u.phase.clone()), Some(DaemonUpdatePhase::Stuck));
    assert_eq!(asks(&c, &target), 1, "{:#?}", steps(&c, &target));
    assert!(t.state.daemon.unwrap().can_update, "Update now stays the user's choice");
    drop(c);
    hub.shutdown();
}

#[test]
fn update_now_interrupts_a_running_turn_marked_by_the_update_and_hands_over() {
    let _serial = serial();
    let Some(mut rig) = Rig::new() else { return };
    rig.serve(&rig.old.clone());
    // Connect first knowing no newer build, and start a turn that runs until interrupted.
    rig.use_bundle(&rig.new.clone(), None);
    let (hub, c, target) = rig.hub();
    wait_serving(&c, &target, OLD);
    let thread = start_thread(&c, &rig, &target, "/fake stall");
    c.subscribe(&target, &thread);
    wait_until("the turn running", || running(&c, &target, &thread));

    // Now the hub holds the newer build: it asks at idle, and the running turn holds it back.
    rig.use_bundle(&rig.new.clone(), Some(NEW));
    reconnect_target(&c, &target);
    let t = wait_phase(&c, &target, &DaemonUpdatePhase::Waiting);
    assert_eq!(t.state.daemon.as_ref().map(|d| d.build.as_str()), Some(OLD));
    std::thread::sleep(Duration::from_millis(1000));
    assert!(rig.serving_alive(), "a running turn holds the hand-over at idle back");

    let r = update_now(&c, &target).expect("update now");
    assert_eq!(r["state"], "exiting");
    wait_serving(&c, &target, NEW);
    wait_until("the older daemon exited", || !rig.serving_alive());
    // The notice reaches the hub's cache from the new daemon's journal a moment after it serves.
    let marked = |all: &[Value]| all.iter().any(|i| i["kind"] == "notice" && i["noticeKind"] == "interruptedByUpdate");
    wait_until("the update's notice in the cache", || marked(&items(&c, &target, &thread)));
    let all = items(&c, &target, &thread);
    assert!(marked(&all), "the turn is marked as interrupted by the update: {all:#?}");
    assert!(!running(&c, &target, &thread));
    let log = steps(&c, &target);
    assert!(log.iter().any(|s| s.contains(&format!("asked yaacd {OLD} to hand over now, to {NEW}"))), "{log:#?}");
    drop(c);
    hub.shutdown();
}

#[test]
fn a_pending_approval_survives_and_holds_the_hand_over_until_answered() {
    let _serial = serial();
    let Some(mut rig) = Rig::new() else { return };
    rig.serve(&rig.old.clone());
    rig.use_bundle(&rig.new.clone(), None);
    let (hub, c, target) = rig.hub();
    wait_serving(&c, &target, OLD);
    let thread = start_thread(&c, &rig, &target, "/fake approve make deploy\n/fake say deployed");
    c.subscribe(&target, &thread);
    let pending = |c: &Client| -> Option<String> {
        items(c, &target, &thread)
            .into_iter()
            .find(|i| i["kind"] == "request" && i["state"] == "pending")
            .and_then(|i| i["requestId"].as_str().map(str::to_owned))
    };
    wait_until("the approval pending", || pending(&c).is_some());

    rig.use_bundle(&rig.new.clone(), Some(NEW));
    reconnect_target(&c, &target);
    wait_phase(&c, &target, &DaemonUpdatePhase::Waiting);
    std::thread::sleep(Duration::from_millis(1500));
    assert!(rig.serving_alive(), "a pending approval holds the hand-over at idle back");
    let request = pending(&c).expect("the approval is still pending, never answered by YAAC");
    assert_eq!(asks(&c, &target), 1);

    // The user answers; the turn ends; the daemon is idle and hands over.
    c.routed(&target, "request/respond", json!({
        "opId": "op-answer",
        "threadId": thread,
        "requestId": request,
        "response": { "requestKind": "approval", "decision": "allow" }
    }))
    .expect("respond");
    wait_serving(&c, &target, NEW);
    wait_until("the older daemon exited", || !rig.serving_alive());
    let all = items(&c, &target, &thread);
    let req = all.iter().find(|i| i["kind"] == "request").expect("the approval is in the transcript");
    assert_eq!(req["state"], "resolved", "{req:#?}");
    assert!(all.iter().any(|i| i["kind"] == "agentMessage" && i["text"] == "deployed"), "{all:#?}");
    assert_eq!(asks(&c, &target), 1);
    drop(c);
    hub.shutdown();
}
