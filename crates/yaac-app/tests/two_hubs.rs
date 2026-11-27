//! One phone, two hubs (docs/14-android.md#several-hubs): two Android window controllers
//! (`view_protocol`, hub mode) run at once in one process, each attached over TCP to its own
//! real hub (no targets), each with its own data folder. Each names its own hub in
//! `shell.hub` (the hub's `hubId` and machine name from `hub/hello`, and the address it
//! dials), applies its own hub's stored choices (`hub/uiState/get`), and never shows the
//! other's.
//!
//! The hubs listen on socket files; a plain loopback relay carries each controller's TCP to
//! its hub. Nothing listens on a Tailscale address or on port 47380.
#![cfg(all(unix, feature = "hub"))]

use std::net::TcpListener;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use yaac_app::view_protocol::{Mode, Options, ViewController};
use yaac_hub::client::HubClient;
use yaac_hub::{DefaultConnectors, HubConfig, HubPaths};
use yaac_transport::LocalConfig;

const WAIT: Duration = Duration::from_secs(60);

/// Loopback TCP to the hub's socket file, both ways.
fn relay(socket: PathBuf) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback relay");
    let address = listener.local_addr().unwrap().to_string();
    std::thread::spawn(move || {
        for tcp in listener.incoming().flatten() {
            let Ok(unix) = UnixStream::connect(&socket) else { continue };
            let (mut tcp_in, mut unix_out) = (tcp.try_clone().unwrap(), unix.try_clone().unwrap());
            std::thread::spawn(move || std::io::copy(&mut tcp_in, &mut unix_out));
            let (mut unix_in, mut tcp_out) = (unix, tcp);
            std::thread::spawn(move || std::io::copy(&mut unix_in, &mut tcp_out));
        }
    });
    address
}

struct Hub {
    socket: PathBuf,
    address: String,
    id: String,
}

/// A hub with no targets in `root`, named `host`, with `theme` as its stored choice.
fn hub(root: &Path, host: &str, theme: &str) -> Hub {
    let paths = HubPaths::under(root);
    let socket = paths.socket();
    let local = LocalConfig::from_env(&yaac_hub::config::default_root());
    let mut config = HubConfig::new(yaac_app::BUILD, paths, Arc::new(DefaultConnectors { local, wsl: Default::default() }));
    config.seed_local_target = false;
    config.handle_signals = false;
    config.host_name = host.into();
    let running = std::thread::spawn(move || yaac_hub::run_hub(config));
    let deadline = Instant::now() + WAIT;
    while !socket.exists() {
        assert!(Instant::now() < deadline && !running.is_finished(), "the hub {host} did not start");
        std::thread::sleep(Duration::from_millis(10));
    }
    let side = HubClient::connect(&socket).unwrap();
    let call = |m: &str, p: Value| side.request_blocking(m, p, WAIT).unwrap_or_else(|e| panic!("{m}: {e:?}"));
    let hello = call("hub/hello", json!({ "build": yaac_app::BUILD }));
    assert_eq!(hello["hostName"], host);
    let id = hello["hubId"].as_str().expect("the hub reports its id").to_owned();
    // The id is the install's: asked again, the same.
    let again = HubClient::connect(&socket).unwrap().request_blocking("hub/hello", json!({ "build": yaac_app::BUILD }), WAIT).unwrap();
    assert_eq!(again["hubId"], id.as_str());
    call("hub/uiState/set", json!({ "key": yaac_core::prefs::keys::THEME, "value": theme }));
    Hub { address: relay(socket.clone()), socket, id }
}

fn controller(hub: &Hub, data_dir: PathBuf) -> ViewController {
    ViewController::start(Options {
        mode: Mode::Hub { address: hub.address.clone(), data_dir },
        theme: "dark".into(),
        input: "touch".into(),
        requests: true,
        platform: "android".into(),
        browser_host: false,
    })
    .unwrap()
}

/// The shell view as it stands, from every frame taken so far.
#[derive(Default)]
struct Shell {
    hub: Value,
    theme: Value,
    /// Every frame, as text, to look for the other hub in.
    all: String,
}

impl Shell {
    fn take(&mut self, c: &ViewController) {
        while let Some(frame) = c.wait_frame(Duration::from_millis(20)) {
            let v: Value = serde_json::from_str(&frame).unwrap();
            if let Some(shell) = v.pointer("/views/shell") {
                if let Some(h) = shell.get("hub") {
                    self.hub = h.clone();
                }
                if let Some(t) = shell.get("theme") {
                    self.theme = t.clone();
                }
            }
            self.all.push_str(&frame);
        }
    }
}

#[test]
fn two_controllers_attach_to_two_hubs_at_once_without_cross_talk() {
    let dir = tempfile::Builder::new().prefix("y2").tempdir_in("/tmp").unwrap();
    let home = hub(&dir.path().join("home"), "HOME-PC", "light");
    let work = hub(&dir.path().join("work"), "WORK-PC", "dark");
    assert_ne!(home.id, work.id, "two installs, two ids");

    let (home_dir, work_dir) = (dir.path().join("phone"), dir.path().join("phone/hubs/1"));
    let (a, b) = (controller(&home, home_dir), controller(&work, work_dir));
    let (mut sa, mut sb) = (Shell::default(), Shell::default());
    let deadline = Instant::now() + WAIT;
    loop {
        sa.take(&a);
        sb.take(&b);
        let done = |s: &Shell| s.hub["id"].is_string() && s.theme.is_string();
        if done(&sa) && done(&sb) && sa.theme == "light" {
            break;
        }
        assert!(Instant::now() < deadline, "the controllers did not attach: {:?} / {:?}", sa.hub, sb.hub);
    }

    assert_eq!(sa.hub["id"], home.id.as_str());
    assert_eq!(sa.hub["name"], "HOME-PC");
    assert_eq!(sa.hub["address"], home.address.as_str());
    assert_eq!(sb.hub["id"], work.id.as_str());
    assert_eq!(sb.hub["name"], "WORK-PC");
    assert_eq!(sb.hub["address"], work.address.as_str());
    // Each applies its own hub's stored theme.
    assert_eq!(sa.theme, "light");
    assert_eq!(sb.theme, "dark");
    // Neither shows the other hub anywhere.
    assert!(!sa.all.contains(&work.id) && !sa.all.contains("WORK-PC"));
    assert!(!sb.all.contains(&home.id) && !sb.all.contains("HOME-PC"));

    // A choice made on one phone controller goes to its own hub only.
    a.command(&json!({ "command": "settings.action", "payload": { "id": "appearance.theme", "values": { "theme": "dark" } } }).to_string()).unwrap();
    b.command(&json!({ "command": "settings.action", "payload": { "id": "appearance.theme", "values": { "theme": "light" } } }).to_string()).unwrap();
    let stored = |h: &Hub| {
        let c = HubClient::connect(&h.socket).unwrap();
        c.request_blocking("hub/hello", json!({ "build": yaac_app::BUILD }), WAIT).unwrap();
        c.request_blocking("hub/uiState/get", json!({ "keys": [yaac_core::prefs::keys::THEME] }), WAIT).unwrap()["values"]
            [yaac_core::prefs::keys::THEME]
            .clone()
    };
    let deadline = Instant::now() + WAIT;
    while !(stored(&home) == "dark" && stored(&work) == "light") {
        sa.take(&a);
        sb.take(&b);
        assert!(Instant::now() < deadline, "the theme choices did not reach their own hubs");
    }

    a.stop();
    b.stop();
}
