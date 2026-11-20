//! End to end on a Windows host: a hub connects to a WSL distro through the WSL connector,
//! stage 0 installs the embedded `yaacd` bundle in a **temporary** `YAAC_HOME` inside the
//! distro, `provider/list` answers, and a turn completes on the fake provider — with the
//! real Codex and Claude providers switched off, so the user's harnesses are never touched.
//!
//! Runs only when `YAAC_TEST_WSL_DISTRO` names a running distro (it never starts one). With
//! `YAAC_TEST_EXE` pointing at `yaac.exe`, the hub is that executable (`yaac.exe hub` in a
//! temporary profile); otherwise it runs in this process. Everything is removed afterwards:
//! the hub's profile, the daemon's home in the distro, and the daemon itself.

#![cfg(windows)]
#![allow(clippy::needless_pass_by_value, reason = "test helpers take JSON by value like the support module")]

use std::ffi::OsStr;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use yaac_hub::cache::Cache;
use yaac_hub::client::HubClient;
use yaac_hub::{DefaultConnectors, HubConfig, HubPaths, WslDefaults, embedded_bundle};
use yaac_protocol::attach::{self, ConnectionStatus, TargetEntry, TargetsListResult};
use yaac_protocol::{ItemBody, TargetId, ThreadId};
use yaac_transport::LocalConfig;
use yaac_transport::wsl::{running_distros, wsl_command};

const BUILD: &str = env!("CARGO_PKG_VERSION");
const WAIT: Duration = Duration::from_secs(90);

fn call(c: &HubClient, method: &str, params: Value) -> Value {
    c.request_blocking(method, params, WAIT).unwrap_or_else(|e| panic!("{method}: {e:?}"))
}

fn routed(c: &HubClient, target: &TargetId, method: &str, params: Value) -> Value {
    call(c, attach::names::REQUEST, json!({ "target": target, "method": method, "params": params }))
}

fn in_distro(distro: &str, script: &str) -> String {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let out = rt.block_on(wsl_command(OsStr::new("wsl.exe"), distro, None, &["sh".into(), "-c".into(), script.into()]).output()).unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned()
}

enum Hub {
    Process(Child),
    Thread(std::thread::JoinHandle<()>),
}

#[test]
fn a_windows_hub_runs_a_turn_in_wsl_on_the_fake_provider() {
    let Some(distro) = std::env::var("YAAC_TEST_WSL_DISTRO").ok().filter(|d| !d.is_empty()) else {
        return eprintln!("skipped: YAAC_TEST_WSL_DISTRO not set");
    };
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    if !rt.block_on(running_distros(OsStr::new("wsl.exe"))).unwrap().iter().any(|d| d.eq_ignore_ascii_case(&distro)) {
        return eprintln!("skipped: {distro} is not running (this test never starts it)");
    }
    assert!(embedded_bundle().is_some(), "this hub was built without daemon/dist/yaacd.mjs");

    let profile = tempfile::Builder::new().prefix("yaac-e2e").tempdir().unwrap();
    let paths = HubPaths::under(profile.path());
    std::fs::create_dir_all(&paths.config_dir).unwrap();
    std::fs::write(paths.settings_file(), r#"{"targets":[],"hub":{"always":false,"idleExitSecs":300}}"#).unwrap();
    let daemon_home = format!("/tmp/yaac-e2e-{}", std::process::id());
    let target_env = format!(
        "YAAC_HOME={daemon_home};YAAC_FAKE_PROVIDER=1;YAAC_CODEX_PROVIDER=0;YAAC_CLAUDE_PROVIDER=0;YAAC_IDLE_TIMEOUT_MS=5000"
    );

    let hub = match std::env::var_os("YAAC_TEST_EXE") {
        Some(exe) => {
            eprintln!("hub: {} hub", Path::new(&exe).display());
            Hub::Process(
                Command::new(exe)
                    .arg("hub")
                    .env("YAAC_CONFIG_DIR", &paths.config_dir)
                    .env("YAAC_DATA_DIR", &paths.data_dir)
                    .env("YAAC_RUNTIME_DIR", &paths.runtime_dir)
                    .env("YAAC_TARGET_ENV", &target_env)
                    .env_remove("YAAC_DAEMON_BUNDLE")
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::inherit())
                    .spawn()
                    .expect("yaac.exe hub"),
            )
        }
        None => {
            eprintln!("hub: in this process");
            let wsl = WslDefaults::from_lookup(|k| (k == "YAAC_TARGET_ENV").then(|| target_env.clone().into()));
            let local = LocalConfig::from_lookup(Path::new("."), |_| None);
            let mut config = HubConfig::new(BUILD, paths.clone(), Arc::new(DefaultConnectors { local, wsl }));
            config.handle_signals = false;
            Hub::Thread(std::thread::spawn(move || {
                yaac_hub::run_hub(config).expect("hub");
            }))
        }
    };

    let socket = paths.socket();
    let deadline = Instant::now() + WAIT;
    let c = loop {
        if let Ok(c) = HubClient::connect(&socket) {
            break c;
        }
        assert!(Instant::now() < deadline, "the hub never listened on {}", socket.display());
        std::thread::sleep(Duration::from_millis(50));
    };
    call(&c, attach::names::HELLO, json!({ "build": BUILD }));
    let added = call(&c, attach::names::TARGETS_ADD, json!({ "name": "wsl", "transport": { "kind": "wsl", "distro": distro }, "enabled": true }));
    let target: TargetEntry = serde_json::from_value(added["target"].clone()).unwrap();
    let target = target.id;

    let t0 = Instant::now();
    loop {
        let list: TargetsListResult = serde_json::from_value(call(&c, attach::names::TARGETS_LIST, json!({}))).unwrap();
        let t = list.targets.into_iter().find(|t| t.id == target).unwrap();
        if t.state.status == ConnectionStatus::Connected {
            eprintln!("connected in {:?}: daemon {:?}", t0.elapsed(), t.state.daemon);
            break;
        }
        assert!(t0.elapsed() < WAIT, "never connected: {:?}", t.state);
        std::thread::sleep(Duration::from_millis(100));
    }
    let installed = in_distro(&distro, &format!("ls '{daemon_home}/daemon'"));
    eprintln!("installed in {daemon_home}/daemon: {}", installed.trim());
    assert!(installed.contains(&embedded_bundle().unwrap().build), "the embedded build was installed in the temporary home");

    let providers = routed(&c, &target, "provider/list", json!({}));
    let instances: Vec<&str> = providers["providers"].as_array().unwrap().iter().filter_map(|p| p["instance"].as_str()).collect();
    eprintln!("provider/list: {instances:?}");
    assert_eq!(instances.len(), 1, "only the fake provider: {providers}");

    let started = routed(&c, &target, "thread/start", json!({
        "opId": "op-wsl-e2e-1",
        "instance": instances[0],
        "cwd": daemon_home,
        "input": [{ "type": "text", "text": "/fake say hello from inside WSL" }]
    }));
    let thread = started["threadId"].as_str().expect("threadId").to_owned();
    routed(&c, &target, "thread/subscribe", json!({ "threadId": thread }));
    let deadline = Instant::now() + WAIT;
    loop {
        let done = Cache::open_read_only(&paths.cache_file()).ok().and_then(|mut cache| {
            let t = cache.thread(&target, &ThreadId::new(&thread)).ok()??;
            let text: String = t.items.iter().filter_map(|i| match &i.body {
                ItemBody::AgentMessage(m) => Some(m.text.clone()),
                _ => None,
            }).collect();
            let live = serde_json::to_value(&t.live).ok()?;
            Some((text, live.get("lastTurn").cloned()))
        });
        if let Some((text, Some(last))) = &done
            && text.contains("hello from inside WSL")
        {
            eprintln!("turn completed: {text:?}, last turn {last}");
            break;
        }
        assert!(Instant::now() < deadline, "the turn never completed: {done:?}");
        std::thread::sleep(Duration::from_millis(100));
    }

    // Stop the hub (which kills its wsl.exe child), then the daemon, then remove its home.
    call(&c, attach::names::SHUTDOWN, json!({}));
    drop(c);
    match hub {
        Hub::Process(mut child) => {
            let deadline = Instant::now() + WAIT;
            while child.try_wait().unwrap().is_none() {
                assert!(Instant::now() < deadline, "yaac.exe hub did not exit");
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        Hub::Thread(t) => t.join().unwrap(),
    }
    let cleanup = in_distro(
        &distro,
        &format!("p=$(cat '{daemon_home}/run/yaacd.pid' 2>/dev/null) && kill \"$p\" 2>/dev/null; sleep 0.5; rm -rf '{daemon_home}' && echo removed"),
    );
    assert_eq!(cleanup.trim(), "removed");
}
