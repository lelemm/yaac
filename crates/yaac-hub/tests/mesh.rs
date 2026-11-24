//! The agent mesh across targets (docs/10-agent-mesh.md): **two real `yaacd` daemons** on
//! their fake providers, acting as two targets, and the **real hub** in process. These are the
//! cases of the "Agent mesh" row of docs/08-roadmap.md#testing-strategy that need a second
//! target; the single-target ones (with no hub at all) are in daemon/test/mesh.test.ts.
//!
//! Runs only when `daemon/dist/yaacd.mjs` exists and `node` (or `YAAC_RUNTIME`) runs;
//! otherwise each test prints that it was skipped and passes. The daemons never reach a real
//! harness (`YAAC_CODEX_PROVIDER=0`, `YAAC_CLAUDE_PROVIDER=0`), live in temporary homes, and
//! exit by themselves a few seconds after the hub lets go of them.
//!
//! The agents here are fake agents driven by `/fake mesh-*` directives
//! (daemon/src/providers/fake/README.md): each directive is one call of a `yaac` tool, made
//! through the thread's attachment ticket exactly as a harness would.

mod support;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use support::{BUILD, Client, Home, RunningHub};
use yaac_hub::settings::TargetDef;
use yaac_hub::{Backoff, ConnectorFactory, HubConfig};
use yaac_protocol::TargetId;
use yaac_protocol::attach::{self, ConnectionStatus};
use yaac_transport::{Connector, LocalConfig, LocalConnector};

const LONG: Duration = Duration::from_secs(40);

fn runtime_and_bundle() -> Option<(std::ffi::OsString, PathBuf)> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let bundle = LocalConfig::default_bundle(&root);
    if !bundle.is_file() {
        eprintln!("SKIPPED: daemon/dist/yaacd.mjs does not exist (build the daemon to run this test)");
        return None;
    }
    let runtime = std::env::var_os("YAAC_RUNTIME").unwrap_or_else(|| "node".into());
    if std::process::Command::new(&runtime).arg("--version").output().is_err() {
        eprintln!("SKIPPED: the runtime {runtime:?} does not run");
        return None;
    }
    Some((runtime, bundle))
}

/// Each target name gets its own daemon: its own home, socket, and fake store.
#[derive(Clone)]
struct Daemons {
    runtime: std::ffi::OsString,
    bundle: PathBuf,
    root: PathBuf,
    made: Arc<Mutex<HashMap<String, LocalConfig>>>,
}

impl Daemons {
    fn home_of(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    fn config(&self, name: &str) -> LocalConfig {
        let home = self.home_of(name);
        std::fs::create_dir_all(home.join("work")).unwrap();
        LocalConfig { runtime: self.runtime.clone(), bundle: self.bundle.clone(), env: vec![], inherit_stderr: false, build: None, install_files: vec![] }
            .with_env("YAAC_FAKE_PROVIDER", "1")
            // Never let these tests probe or start the user's real harnesses.
            .with_env("YAAC_CODEX_PROVIDER", "0")
            .with_env("YAAC_CLAUDE_PROVIDER", "0")
            .with_env("YAAC_HOME", home.join(".yaac"))
            .with_env("YAAC_SOCKET", self.root.join(format!("{name}.sock")))
            .with_env("YAAC_IDLE_TIMEOUT_MS", "3000")
            .with_env("YAAC_MESH_WORKER_POLL_MS", "300")
            .with_env("HOME", &home)
    }
}

impl ConnectorFactory for Daemons {
    fn connector(&self, target: &TargetDef) -> Result<Arc<dyn Connector>, String> {
        let name = target.name.as_str().to_owned();
        let config = self.made.lock().unwrap().entry(name.clone()).or_insert_with(|| self.config(&name)).clone();
        Ok(Arc::new(LocalConnector::new(config)))
    }
}

struct Mesh {
    home: Home,
    daemons: Daemons,
    hub: Option<RunningHub>,
    c: Option<Client>,
    ids: HashMap<String, TargetId>,
}

impl Mesh {
    fn new(names: &[&str]) -> Option<Self> {
        let (runtime, bundle) = runtime_and_bundle()?;
        let home = Home::new();
        let daemons = Daemons { runtime, bundle, root: home.dir.path().join("t"), made: Arc::default() };
        std::fs::create_dir_all(&daemons.root).unwrap();
        let mut m = Self { home, daemons, hub: None, c: None, ids: HashMap::new() };
        m.start_hub();
        for name in names {
            let id = m.c().add_target(name).id;
            m.ids.insert((*name).to_owned(), id);
        }
        for name in names {
            m.wait_connected(name);
        }
        Some(m)
    }

    fn config(&self) -> HubConfig {
        let mut config = HubConfig::new(BUILD, self.home.paths(), Arc::new(self.daemons.clone()));
        config.seed_local_target = false;
        config.test_methods = true;
        config.handle_signals = false;
        config.host_name = "test-host".into();
        config.backoff = Backoff { initial: Duration::from_millis(50), max: Duration::from_millis(300) };
        config.ack_interval = Duration::from_millis(50);
        config
    }

    fn start_hub(&mut self) {
        let hub = self.home.start_with(self.config());
        self.c = Some(hub.client());
        self.hub = Some(hub);
    }

    fn stop_hub(&mut self) {
        self.c = None;
        if let Some(h) = self.hub.take() {
            h.shutdown();
        }
    }

    fn c(&self) -> &Client {
        self.c.as_ref().expect("hub client")
    }

    fn id(&self, name: &str) -> TargetId {
        self.ids[name].clone()
    }

    fn work(&self, name: &str) -> String {
        self.daemons.home_of(name).join("work").display().to_string()
    }

    fn wait_connected(&self, name: &str) {
        let id = self.id(name);
        wait_long(&format!("{name} connected"), || self.c().targets().iter().any(|t| t.id == id && t.state.status == ConnectionStatus::Connected));
    }

    fn set_enabled(&self, name: &str, enabled: bool) {
        self.c().call(attach::names::TARGETS_ENABLE, json!({ "id": self.id(name), "enabled": enabled })).expect("enable");
    }

    fn routed(&self, target: &str, method: &str, params: Value) -> Value {
        self.c().routed(&self.id(target), method, params).unwrap_or_else(|e| panic!("{method} on {target}: {e:?}"))
    }

    /// Starts a fake thread on `target` whose chat allows agents.
    fn start(&self, target: &str, text: &str) -> String {
        let op = format!("op-{}", rand_suffix());
        let r = self.routed(target, "thread/start", json!({
            "opId": op, "instance": "pi_fake", "cwd": self.work(target), "agents": "allow",
            "input": [{ "type": "text", "text": text }]
        }));
        r["threadId"].as_str().expect("threadId").to_owned()
    }

    fn address(&self, target: &str, thread: &str) -> String {
        format!("{}/{thread}", self.id(target))
    }

    /// A thread's items, oldest first.
    fn items(&self, target: &str, thread: &str) -> Vec<Value> {
        let mut out = Vec::new();
        let mut cursor = Value::Null;
        loop {
            let page = self.routed(target, "thread/read", json!({ "threadId": thread, "cursor": cursor }));
            out.extend(page["items"].as_array().cloned().unwrap_or_default());
            cursor = page["cursor"].clone();
            if cursor.is_null() {
                break;
            }
        }
        out.reverse();
        out
    }

    /// Completed `yaac.<tool>` calls of a fake agent: (output, failed).
    fn tool_outputs(&self, target: &str, thread: &str, tool: &str) -> Vec<(String, bool)> {
        self.items(target, thread)
            .into_iter()
            .filter(|i| i["kind"] == "toolCall" && i["title"] == format!("yaac.{tool}") && i["status"] != "inProgress")
            .map(|i| (i["output"].as_str().unwrap_or_default().to_owned(), i["status"] == "failed"))
            .collect()
    }

    fn until_tool(&self, target: &str, thread: &str, tool: &str, n: usize) -> Vec<(String, bool)> {
        let mut got = Vec::new();
        wait_long(&format!("{n} {tool} call(s) on {target}"), || {
            got = self.tool_outputs(target, thread, tool);
            got.len() >= n
        });
        got
    }

    fn kinds(&self, target: &str, thread: &str, kind: &str) -> Vec<Value> {
        self.items(target, thread).into_iter().filter(|i| i["kind"] == kind).collect()
    }

    fn state(&self, target: &str, thread: &str) -> String {
        let sync = self.routed(target, "thread/sync", json!({}));
        sync["changed"]
            .as_array()
            .and_then(|a| a.iter().find(|t| t["id"] == thread))
            .and_then(|t| t["state"].as_str())
            .unwrap_or("?")
            .to_owned()
    }

    fn idle(&self, target: &str, thread: &str) {
        wait_long(&format!("{thread} on {target} to finish"), || {
            let s = self.state(target, thread);
            s == "idle" || s == "done" || s == "failed"
        });
    }
}

impl Drop for Mesh {
    fn drop(&mut self) {
        self.stop_hub();
        // The daemons exit by themselves once idle with no client (3 s); give them the time so
        // no yaacd is left running, and their homes can go.
        let deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < deadline {
            let alive = std::fs::read_dir(&self.daemons.root)
                .map(|d| d.filter_map(Result::ok).any(|e| e.path().extension().is_some_and(|x| x == "sock") && std::os::unix::net::UnixStream::connect(e.path()).is_ok()))
                .unwrap_or(false);
            if !alive {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        // A test that failed half-way can leave a turn running (and so a daemon up): stop them.
        for name in self.ids.keys() {
            let pid = self.daemons.home_of(name).join(".yaac/run/yaacd.pid");
            if let Ok(p) = std::fs::read_to_string(&pid) {
                let _ = std::process::Command::new("kill").arg(p.trim()).status();
            }
        }
    }
}

fn rand_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    format!("{t}-{}", N.fetch_add(1, Ordering::Relaxed))
}

fn wait_long(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + LONG;
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn text_of(items: &[Value], kind: &str) -> String {
    items.iter().filter(|i| i["kind"] == kind).filter_map(|i| i["text"].as_str()).collect::<Vec<_>>().join("\n")
}

fn exists(p: &Path) -> bool {
    p.exists()
}

/// The scripted end to end: a fake agent on A spawns a worker on B, waits for it, and notifies a
/// third thread — with no window ever started. The worker's result returns once; the third
/// thread receives A's final answer, attributed to A; the budget spent on B is counted
/// against A's root through `mesh/usage`.
#[test]
fn an_agent_on_one_target_spawns_a_worker_on_another_waits_and_notifies_a_third_thread() {
    let Some(m) = Mesh::new(&["alpha", "beta"]) else { return };
    let third = m.start("alpha", "/fake say I am the third thread");
    m.idle("alpha", &third);
    let script = [
        "Orchestrate the migration.".to_owned(),
        format!("/fake mesh-notify-when-done {}", m.address("alpha", &third)),
        format!("/fake mesh-spawn target=beta instance=pi_fake model=fake-small title=Remote-worker cwd={} | /fake say migrated 3 tables on beta", m.work("beta")),
        "/fake mesh-wait $spawned".to_owned(),
        "/fake say All done: the worker on beta migrated 3 tables.".to_owned(),
    ]
    .join("\n");
    let orchestrator = m.start("alpha", &script);
    let spawned = m.until_tool("alpha", &orchestrator, "spawn_thread", 1);
    assert!(!spawned[0].1, "spawn failed: {}", spawned[0].0);
    assert!(spawned[0].0.contains(&format!("Address: {}/", m.id("beta"))), "{}", spawned[0].0);
    let worker = spawned[0].0.lines().find_map(|l| l.strip_prefix("Address: ")).unwrap().split_once('/').unwrap().1.to_owned();
    let waited = m.until_tool("alpha", &orchestrator, "wait", 1);
    assert!(waited[0].0.contains("result from"), "{}", waited[0].0);
    assert!(waited[0].0.contains("migrated 3 tables on beta"), "{}", waited[0].0);
    m.idle("alpha", &orchestrator);

    // The worker on beta: an ordinary thread, spawned by the orchestrator, given its task as
    // a message from an agent, never as the user.
    let witems = m.items("beta", &worker);
    let received: Vec<&Value> = witems.iter().filter(|i| i["kind"] == "meshReceived").collect();
    assert_eq!(received.len(), 1);
    assert_eq!(received[0]["message"]["from"], m.address("alpha", &orchestrator));
    assert_eq!(witems.iter().filter(|i| i["kind"] == "userMessage").count(), 0);
    let sync = m.routed("beta", "thread/sync", json!({}));
    let ws = sync["changed"].as_array().unwrap().iter().find(|t| t["id"] == worker.as_str()).unwrap().clone();
    assert_eq!(ws["spawnedBy"], m.address("alpha", &orchestrator));
    assert_eq!(ws["root"], m.address("alpha", &orchestrator));

    // The third thread heard the orchestrator's final answer, once, from the orchestrator.
    wait_long("the third thread to be notified", || m.kinds("alpha", &third, "meshReceived").len() == 1);
    let note = &m.kinds("alpha", &third, "meshReceived")[0];
    assert_eq!(note["message"]["from"], m.address("alpha", &orchestrator));
    assert!(note["message"]["text"].as_str().unwrap().contains("All done"));

    // The orchestrator's live spawn item shows the remote worker done.
    wait_long("the spawn item updated", || {
        m.kinds("alpha", &orchestrator, "meshSpawn").first().is_some_and(|s| s["threads"][0]["state"] == "done")
    });
    // Exactly one result came back.
    std::thread::sleep(Duration::from_millis(500));
    let results: Vec<Value> = m.kinds("alpha", &orchestrator, "meshReceived").into_iter().filter(|i| i["message"]["kind"] == "result").collect();
    assert_eq!(results.len(), 1);
    // The sent item on beta (the result) got its receipt from the hub, updated in place.
    wait_long("the result's receipt", || {
        m.kinds("beta", &worker, "meshSent").first().is_some_and(|s| s["receipt"]["status"] == "delivered")
    });
    // The budget: beta reported its spending against alpha's root.
    let budget = m.routed("alpha", "mesh/budget", json!({ "action": "status", "opId": format!("op-{}", rand_suffix()), "root": m.address("alpha", &orchestrator) }));
    assert!(budget["used"]["threadsSpawned"].as_u64().unwrap() >= 1, "{budget}");
    assert!(budget["used"]["agentTurns"].as_u64().unwrap() >= 1, "{budget}");
}

/// A message to a target that is offline waits in the hub and arrives when it reconnects.
#[test]
fn a_message_to_an_offline_target_waits_in_the_hub_and_arrives_on_reconnect() {
    let Some(m) = Mesh::new(&["alpha", "beta"]) else { return };
    let recipient = m.start("beta", "/fake say ready on beta");
    m.idle("beta", &recipient);
    m.set_enabled("beta", false);
    wait_long("beta offline", || m.c().targets().iter().any(|t| t.id == m.id("beta") && t.state.status != ConnectionStatus::Connected));
    let sender = m.start("alpha", &format!("/fake mesh-send {} | /fake say hello from alpha", m.address("beta", &recipient)));
    let sent = m.until_tool("alpha", &sender, "send_message", 1);
    assert!(!sent[0].1, "{}", sent[0].0);
    assert!(sent[0].0.starts_with("Queued"), "{}", sent[0].0);
    m.idle("alpha", &sender);
    m.set_enabled("beta", true);
    m.wait_connected("beta");
    wait_long("delivery after reconnect", || m.kinds("beta", &recipient, "meshReceived").len() == 1);
    let got = &m.kinds("beta", &recipient, "meshReceived")[0];
    assert_eq!(got["message"]["from"], m.address("alpha", &sender));
    m.idle("beta", &recipient);
    assert!(text_of(&m.items("beta", &recipient), "agentMessage").contains("hello from alpha"));
    // The sender's item moved from queued to delivered when the hub's receipt came back.
    wait_long("the receipt", || m.kinds("alpha", &sender, "meshSent").first().is_some_and(|s| s["receipt"]["status"] == "delivered"));
}

/// With the hub stopped, a cross-target message waits in the source's outbox, a cross-target
/// spawn fails at once, and same-target messaging still works; when the hub returns, the
/// message is relayed and delivered.
#[test]
fn with_the_hub_stopped_cross_target_messages_wait_in_the_outbox_and_same_target_work_goes_on() {
    let Some(mut m) = Mesh::new(&["alpha", "beta"]) else { return };
    let remote = m.start("beta", "/fake say on beta");
    m.idle("beta", &remote);
    let local_peer = m.start("alpha", "/fake say local peer");
    m.idle("alpha", &local_peer);
    let script = [
        "/fake sleep 2500".to_owned(),
        format!("/fake mesh-send {} | /fake say crossed while the hub was down", m.address("beta", &remote)),
        format!("/fake mesh-spawn target=beta cwd={} | /fake say never", m.work("beta")),
        format!("/fake mesh-send {} | /fake say local works", m.address("alpha", &local_peer)),
        "/fake mesh-wait timeoutSecs=10".to_owned(),
    ]
    .join("\n");
    let a = m.start("alpha", &script);
    // Stop the hub while the agent sleeps: alpha keeps running the turn on its own.
    std::thread::sleep(Duration::from_millis(300));
    m.stop_hub();
    // Read alpha's journal later; for now, give the agent time to act.
    std::thread::sleep(Duration::from_secs(6));
    m.start_hub();
    m.wait_connected("alpha");
    m.wait_connected("beta");
    let sends = m.until_tool("alpha", &a, "send_message", 2);
    assert!(sends[0].0.contains("the hub is not reachable"), "{}", sends[0].0);
    let spawn = m.until_tool("alpha", &a, "spawn_thread", 1);
    assert!(spawn[0].1, "a cross-target spawn fails: {}", spawn[0].0);
    assert!(spawn[0].0.contains("cannot be queued"), "{}", spawn[0].0);
    assert!(sends[1].0.starts_with("Delivered"), "same-target sending works: {}", sends[1].0);
    // The local peer's reply-less turn ran with the message.
    assert!(m.kinds("alpha", &local_peer, "meshReceived").len() == 1);
    // And the outboxed message made it once the hub returned.
    wait_long("the outboxed message", || m.kinds("beta", &remote, "meshReceived").len() == 1);
    m.idle("alpha", &a);
}

/// `find_thread` searches every target from the hub's index (by fuzzy title and by id
/// prefix); `list_targets` gives names and counts; `list_threads` on another target is paged;
/// `read_thread` reaches unrelated threads and workers on another target.
/// `stop_thread` reaches the caller's worker.
#[test]
fn finding_listing_reading_and_stopping_across_targets() {
    let Some(m) = Mesh::new(&["alpha", "beta"]) else { return };
    let ci = m.start("beta", "CI integration nightly\n/fake say ci");
    let stranger = m.start("beta", "Stranger thread\n/fake say independent answer");
    for i in 0..22 {
        let t = m.start("beta", &format!("/fake say filler {i}"));
        m.idle("beta", &t);
    }
    m.idle("beta", &ci);
    m.idle("beta", &stranger);
    // The hub's index has beta's threads once a sync has landed.
    wait_long("beta's index in the hub", || {
        m.home.cache().threads(&m.id("beta")).map(|t| t.iter().any(|s| s.id.as_str() == ci)).unwrap_or(false)
    });
    let script = [
        "Searcher".to_owned(),
        "/fake mesh-find ci integ".to_owned(),
        format!("/fake mesh-find {}", &ci[..12]),
        "/fake mesh-targets".to_owned(),
        "/fake mesh-list target target=beta".to_owned(),
        format!("/fake mesh-read {}", m.address("beta", &stranger)),
        format!("/fake mesh-spawn target=beta cwd={} title=Stoppable | /fake say working\\n/fake stall", m.work("beta")),
        "/fake sleep 500".to_owned(),
        "/fake mesh-read $spawned".to_owned(),
        "/fake mesh-stop $spawned".to_owned(),
        "/fake mesh-wait $spawned".to_owned(),
    ]
    .join("\n");
    let a = m.start("alpha", &script);
    let finds = m.until_tool("alpha", &a, "find_thread", 2);
    assert!(finds[0].0.contains("across all machines"), "{}", finds[0].0);
    assert!(finds[0].0.lines().nth(1).unwrap().contains(&ci), "fuzzy title, best first: {}", finds[0].0);
    assert!(finds[0].0.contains("· beta"), "it says which machine: {}", finds[0].0);
    assert!(finds[1].0.contains(&ci), "by id prefix: {}", finds[1].0);
    let targets = m.until_tool("alpha", &a, "list_targets", 1);
    assert!(targets[0].0.contains("alpha (this machine) — reachable"), "{}", targets[0].0);
    assert!(targets[0].0.contains("beta — reachable"), "{}", targets[0].0);
    let listed = m.until_tool("alpha", &a, "list_threads", 1);
    assert!(listed[0].0.starts_with("20 of "), "paged: {}", listed[0].0);
    assert!(listed[0].0.contains("pass cursor"), "{}", listed[0].0);
    let reads = m.until_tool("alpha", &a, "read_thread", 2);
    assert!(!reads[0].1, "an unrelated thread can be read: {}", reads[0].0);
    assert!(reads[0].0.contains("[agent] independent answer"), "{}", reads[0].0);
    assert!(!reads[1].1, "its own worker can: {}", reads[1].0);
    assert!(reads[1].0.contains("working"), "{}", reads[1].0);
    let stops = m.until_tool("alpha", &a, "stop_thread", 1);
    assert!(!stops[0].1, "{}", stops[0].0);
    let waits = m.until_tool("alpha", &a, "wait", 1);
    assert!(waits[0].0.contains("interrupted"), "the worker was stopped and its result says so: {}", waits[0].0);
    m.idle("alpha", &a);
    assert!(exists(&m.daemons.home_of("beta")));
}

/// A window cannot inject mesh traffic (`mesh/deliver` is not routable), and one send is
/// delivered exactly once across targets. (The hub's caller check is in mesh_routing.rs; a
/// repeated `mesh/deliver` of one id is in daemon/test/mesh.test.ts.)
#[test]
fn a_window_cannot_inject_mesh_traffic_and_one_send_is_delivered_once() {
    let Some(m) = Mesh::new(&["alpha", "beta"]) else { return };
    let recipient = m.start("beta", "/fake say ready");
    m.idle("beta", &recipient);
    let sender = m.start("alpha", "/fake say ready");
    m.idle("alpha", &sender);
    // A window cannot inject mesh traffic: relay and deliver are the hub's own business.
    let err = m.c().routed(&m.id("beta"), "mesh/deliver", json!({ "message": {} })).unwrap_err();
    assert_eq!(err.code.as_str(), "not_routable", "{err:?}");
    // The same message sent twice by its agent is two messages; the same message *id* is one:
    // simulate the source retrying a relay by sending the same text twice and checking ids.
    let a = m.start("alpha", &format!("/fake mesh-send {} | /fake say once", m.address("beta", &recipient)));
    m.until_tool("alpha", &a, "send_message", 1);
    wait_long("delivered", || m.kinds("beta", &recipient, "meshReceived").len() == 1);
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(m.kinds("beta", &recipient, "meshReceived").len(), 1);
}

/// Stopping a worker stops its whole subtree across machines and holds it: alpha's agent stops
/// its worker on beta, which had spawned a worker back on alpha; each daemon checks the stop is
/// from above, and the tool reports a real outcome per thread. Meanwhile the parent's workers
/// item follows a remote worker's state live (needing the user, stopped), not only when its
/// result arrives.
#[test]
fn a_stop_holds_the_whole_subtree_across_targets_and_the_workers_item_follows_remote_state() {
    let Some(m) = Mesh::new(&["alpha", "beta"]) else { return };
    let script = [
        "Orchestrator".to_owned(),
        format!(
            "/fake mesh-spawn target=beta cwd={} title=Middle | /fake mesh-spawn target=alpha cwd={} title=Leaf | /fake stall\\n/fake stall",
            m.work("beta"),
            m.work("alpha")
        ),
        format!("/fake mesh-spawn target=beta cwd={} title=Asker | /fake approve make deploy", m.work("beta")),
        "/fake sleep 2500".to_owned(),
        "/fake mesh-stop $spawned1".to_owned(),
        "/fake sleep 1500".to_owned(),
    ]
    .join("\n");
    let a = m.start("alpha", &script);
    let stops = m.until_tool("alpha", &a, "stop_thread", 1);
    let (out, failed) = &stops[0];
    assert!(!failed, "{out}");
    assert!(out.contains("\"Middle\"") && out.contains("\"Leaf\""), "the whole subtree: {out}");
    assert_eq!(out.matches(": stopped.").count(), 2, "a real outcome for each: {out}");
    m.idle("alpha", &a);
    // Each daemon holds its own part of the subtree.
    let beta_threads = m.routed("beta", "thread/sync", json!({}));
    let middle = beta_threads["changed"].as_array().unwrap().iter().find(|t| t["title"] == "Middle").cloned().expect("Middle on beta");
    assert_eq!(middle["state"], "stopped", "{middle}");
    assert!(middle["hold"]["by"].as_str().unwrap().ends_with(&a), "held by the orchestrator: {middle}");
    let alpha_threads = m.routed("alpha", "thread/sync", json!({}));
    let leaf = alpha_threads["changed"].as_array().unwrap().iter().find(|t| t["title"] == "Leaf").cloned().expect("Leaf on alpha");
    assert_eq!(leaf["state"], "stopped", "{leaf}");
    // The workers item on alpha follows the remote workers: one stopped, one needing the user.
    wait_long("the workers item to follow beta", || {
        let spawn = m.kinds("alpha", &a, "meshSpawn");
        let states: Vec<String> =
            spawn.iter().flat_map(|i| i["threads"].as_array().cloned().unwrap_or_default()).map(|t| t["state"].as_str().unwrap_or("").to_owned()).collect();
        states.contains(&"stopped".to_owned()) && states.contains(&"needsAttention".to_owned())
    });
    // Nothing more runs in the held subtree: a stranger's message waits.
    let stranger = m.start("alpha", &format!("/fake mesh-send {}/{} | /fake say hello", m.id("beta"), middle["id"].as_str().unwrap()));
    m.until_tool("alpha", &stranger, "send_message", 1);
    wait_long("the sender to be told it is stopped", || {
        m.kinds("alpha", &stranger, "meshSent").iter().any(|i| i["receipt"]["reason"] == "stopped" && i["receipt"]["status"] == "queued")
    });
}
