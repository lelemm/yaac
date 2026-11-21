//! The hub against the **real** `yaacd` bundle on its fake provider. Runs only when
//! `daemon/dist/yaacd.mjs` exists and `node` (or `YAAC_RUNTIME`) runs; otherwise it prints
//! that it was skipped and passes.
//!
//! ASSUMPTIONS (from `daemon/src/main.ts` and `daemon/src/providers/fake/script.ts`, not yet
//! from a finished `daemon/README.md`):
//! - `YAAC_FAKE_PROVIDER=1` adds an implicit fake provider instance, listed by `provider/list`;
//! - `YAAC_HOME` moves the daemon's state, `YAAC_SOCKET` its socket, `YAAC_IDLE_TIMEOUT_MS` its
//!   idle exit (so the detached daemon this test starts goes away by itself);
//! - message lines `/fake pace <ms>` and `/fake say <text>` make the fake stream `<text>` as
//!   an agent message, slowly enough to drop the connection in the middle.
//!
//! If an assumption does not hold the test fails with a message naming it.

mod support;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use support::{BUILD, Home, wait_until};
use yaac_hub::protocol::names as ext;
use yaac_hub::{DefaultConnectors, HubConfig};
use yaac_protocol::attach::ConnectionStatus;
use yaac_protocol::{ItemBody, ThreadId};
use yaac_transport::LocalConfig;

fn bundle() -> Option<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let bundle = LocalConfig::default_bundle(&root);
    bundle.is_file().then_some(bundle)
}

fn agent_text(items: &[yaac_protocol::Item]) -> String {
    items
        .iter()
        .filter_map(|i| match &i.body {
            ItemBody::AgentMessage(m) => Some(m.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

#[test]
fn a_turn_on_the_real_daemon_survives_a_dropped_connection() {
    let Some(bundle) = bundle() else {
        eprintln!("SKIPPED: daemon/dist/yaacd.mjs does not exist (build the daemon to run this test)");
        return;
    };
    let runtime = std::env::var_os("YAAC_RUNTIME").unwrap_or_else(|| "node".into());
    if std::process::Command::new(&runtime).arg("--version").output().is_err() {
        eprintln!("SKIPPED: the runtime {runtime:?} does not run");
        return;
    }

    let home = Home::new();
    let daemon_home = home.dir.path().join("target-home");
    std::fs::create_dir_all(&daemon_home).unwrap();
    let local = LocalConfig { runtime, bundle, env: vec![], inherit_stderr: false }
        .with_env("YAAC_FAKE_PROVIDER", "1")
        // Never let this test probe or start the user's real harnesses.
        .with_env("YAAC_CODEX_PROVIDER", "0")
        .with_env("YAAC_CLAUDE_PROVIDER", "0")
        .with_env("YAAC_HOME", daemon_home.join(".yaac"))
        .with_env("YAAC_SOCKET", home.dir.path().join("d.sock"))
        .with_env("YAAC_IDLE_TIMEOUT_MS", "3000")
        .with_env("HOME", &daemon_home);
    let mut config = HubConfig::new(BUILD, home.paths(), Arc::new(DefaultConnectors { local, wsl: Default::default() }));
    config.seed_local_target = false;
    config.test_methods = true;
    config.handle_signals = false;
    config.backoff = yaac_hub::Backoff { initial: Duration::from_millis(50), max: Duration::from_millis(500) };
    let hub = home.start_with(config);
    let c = hub.client();
    let target = c.add_target("local").id;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let t = c.targets().into_iter().find(|t| t.id == target).unwrap();
        if t.state.status == ConnectionStatus::Connected {
            break;
        }
        assert!(Instant::now() < deadline, "the real daemon never connected: {:?}", t.state);
        std::thread::sleep(Duration::from_millis(50));
    }

    let providers = c.routed(&target, "provider/list", json!({})).expect("provider/list");
    let instance = providers["providers"]
        .as_array()
        .and_then(|p| p.first())
        .and_then(|p| p["instance"].as_str())
        .map(str::to_owned)
        .expect("ASSUMPTION: YAAC_FAKE_PROVIDER=1 lists a provider instance");

    let words: Vec<String> = (0..120).map(|i| format!("word{i}")).collect();
    let text = format!("/fake pace 15\n/fake say {}", words.join(" "));
    let started = c
        .routed(&target, "thread/start", json!({
            "opId": "op-real-1",
            "instance": instance,
            "cwd": daemon_home.display().to_string(),
            "input": [{ "type": "text", "text": text }]
        }))
        .expect("thread/start");
    let thread = started["threadId"].as_str().expect("threadId").to_owned();
    c.subscribe(&target, &thread);

    let cached = |home: &Home| -> Option<(String, Value)> {
        let t = home.cache().thread(&target, &ThreadId::new(&thread)).ok()??;
        Some((agent_text(&t.items), serde_json::to_value(&t.live).ok()?))
    };
    // Drop the connection while the message is streaming.
    wait_until("streaming started", || cached(&home).is_some_and(|(t, _)| t.contains("word5")));
    c.call(ext::DEBUG_DROP_CONNECTION, json!({ "id": target })).unwrap();
    c.wait_status(&target, ConnectionStatus::Offline);
    c.wait_status(&target, ConnectionStatus::Connected);

    wait_until("turn completed in the cache", || cached(&home).is_some_and(|(t, live)| t.contains("word119") && live.get("lastTurn").is_some()));
    let (cached_text, _) = cached(&home).unwrap();
    let read = c.routed(&target, "thread/read", json!({ "threadId": thread })).expect("thread/read");
    let items: Vec<yaac_protocol::Item> = serde_json::from_value(read["items"].clone()).unwrap();
    let mut items = items;
    items.reverse();
    assert_eq!(cached_text, agent_text(&items), "the cache matches the daemon: nothing lost, nothing doubled");
    assert_eq!(cached_text.matches("word119").count(), 1, "no text applied twice");
    for w in &words {
        assert!(cached_text.contains(w.as_str()), "{w} reached the cache");
    }
    drop(c);
    hub.shutdown();
}
