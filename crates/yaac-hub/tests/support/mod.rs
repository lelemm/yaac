//! Test harness: a hub on its own thread, in a temporary directory, with fake daemons as
//! targets. Nothing touches the real home, config, or data directories.

#![allow(dead_code, unreachable_pub, clippy::needless_pass_by_value, clippy::missing_panics_doc)]

pub mod fake_daemon;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use yaac_hub::cache::Cache;
use yaac_hub::client::HubClient;
use yaac_hub::settings::TargetDef;
use yaac_hub::{Backoff, ConnectorFactory, HubConfig, HubError, HubExit, HubPaths};
use yaac_protocol::attach::{self, ConnectionStatus, TargetEntry, TargetsListResult};
use yaac_protocol::rpc::{ErrorObject, Message};
use yaac_protocol::{TargetId, ThreadId};
use yaac_transport::Connector;

pub use fake_daemon::FakeDaemon;

pub const BUILD: &str = "0.1.0+test";
pub const WAIT: Duration = Duration::from_secs(15);

/// Maps target names to fake daemons.
#[derive(Clone, Default)]
pub struct Fakes(pub Arc<Mutex<HashMap<String, FakeDaemon>>>);

impl Fakes {
    pub fn add(&self, daemon: &FakeDaemon) {
        self.0.lock().unwrap().insert(daemon.name().to_owned(), daemon.clone());
    }
}

impl ConnectorFactory for Fakes {
    fn connector(&self, target: &TargetDef) -> Result<Arc<dyn Connector>, String> {
        let fakes = self.0.lock().unwrap();
        let fake = fakes.get(target.name.as_str()).ok_or_else(|| format!("no fake daemon named {}", target.name))?;
        Ok(Arc::new(fake.clone()))
    }
}

/// A temporary hub home.
pub struct Home {
    pub dir: tempfile::TempDir,
    pub fakes: Fakes,
}

impl Home {
    pub fn new() -> Self {
        // Under /tmp, so the socket path stays short.
        let dir = tempfile::Builder::new().prefix("yh").tempdir_in("/tmp").expect("temp dir");
        Self { dir, fakes: Fakes::default() }
    }

    pub fn paths(&self) -> HubPaths {
        HubPaths::under(self.dir.path())
    }

    pub fn config(&self) -> HubConfig {
        let mut c = HubConfig::new(BUILD, self.paths(), Arc::new(self.fakes.clone()));
        c.backoff = Backoff { initial: Duration::from_millis(20), max: Duration::from_millis(200) };
        c.ack_interval = Duration::from_millis(50);
        c.idle_exit = Some(Duration::from_secs(600));
        c.seed_local_target = false;
        c.test_methods = true;
        c.handle_signals = false;
        c.host_name = "test-host".into();
        c
    }

    pub fn start(&self) -> RunningHub {
        self.start_with(self.config())
    }

    pub fn start_with(&self, config: HubConfig) -> RunningHub {
        let socket = config.paths.socket();
        let thread = std::thread::spawn(move || yaac_hub::run_hub(config));
        let deadline = Instant::now() + WAIT;
        while !socket.exists() {
            assert!(Instant::now() < deadline, "the hub did not start listening");
            if thread.is_finished() {
                panic!("the hub exited at start: {:?}", thread.join());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        RunningHub { thread: Some(thread), socket }
    }

    pub fn cache(&self) -> Cache {
        Cache::open_read_only(&self.paths().cache_file()).expect("cache")
    }
}

pub struct RunningHub {
    pub thread: Option<JoinHandle<Result<HubExit, HubError>>>,
    pub socket: PathBuf,
}

impl RunningHub {
    /// A client that has said hello.
    pub fn client(&self) -> Client {
        let link = HubClient::connect(&self.socket).expect("connect");
        let client = Client { link };
        client.call(attach::names::HELLO, json!({ "build": BUILD })).expect("hello");
        client
    }

    /// Asks the hub to stop and waits for it.
    pub fn shutdown(mut self) -> HubExit {
        let link = HubClient::connect(&self.socket).expect("connect");
        let c = Client { link };
        c.call(attach::names::HELLO, json!({ "build": BUILD })).expect("hello");
        c.call(attach::names::SHUTDOWN, json!({})).expect("shutdown");
        self.join()
    }

    pub fn join(&mut self) -> HubExit {
        self.thread.take().unwrap().join().expect("hub thread").expect("hub result")
    }
}

impl Drop for RunningHub {
    fn drop(&mut self) {
        if self.thread.is_some() && !std::thread::panicking() {
            if let Ok(link) = HubClient::connect(&self.socket) {
                let c = Client { link };
                let _ = c.call(attach::names::HELLO, json!({ "build": BUILD }));
                let _ = c.call(attach::names::SHUTDOWN, json!({}));
            }
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }
    }
}

pub struct Client {
    pub link: HubClient,
}

impl Client {
    pub fn call(&self, method: &str, params: Value) -> Result<Value, ErrorObject> {
        self.link.request_blocking(method, params, WAIT)
    }

    pub fn routed(&self, target: &TargetId, method: &str, params: Value) -> Result<Value, ErrorObject> {
        self.call(attach::names::REQUEST, json!({ "target": target, "method": method, "params": params }))
    }

    pub fn add_target(&self, name: &str) -> TargetEntry {
        let r = self.call(attach::names::TARGETS_ADD, json!({ "name": name, "transport": { "kind": "local" }, "enabled": true })).expect("add");
        serde_json::from_value(r["target"].clone()).expect("entry")
    }

    pub fn targets(&self) -> Vec<TargetEntry> {
        let r: TargetsListResult = serde_json::from_value(self.call(attach::names::TARGETS_LIST, json!({})).unwrap()).unwrap();
        r.targets
    }

    pub fn wait_status(&self, id: &TargetId, status: ConnectionStatus) {
        wait_until(&format!("{id} {status}"), || self.targets().iter().any(|t| &t.id == id && t.state.status == status));
    }

    pub fn subscribe(&self, target: &TargetId, thread: &str) -> Value {
        self.routed(target, "thread/subscribe", json!({ "threadId": thread })).expect("subscribe")
    }

    /// Every message received so far that is not an answer.
    pub fn drain(&self) -> Vec<Message> {
        let mut out = Vec::new();
        while let Some(m) = self.link.try_recv() {
            out.push(m);
        }
        out
    }
}

pub fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The cached text of one agent-message item, and the thread's cached position.
pub fn cached_text(home: &Home, target: &TargetId, thread: &str, item: &str) -> Option<(String, u64, u64)> {
    let mut cache = home.cache();
    let t = cache.thread(target, &ThreadId::new(thread)).ok()??;
    let text = t.items.iter().find(|i| i.id.as_str() == item).and_then(|i| match &i.body {
        yaac_protocol::ItemBody::AgentMessage(m) => Some(m.text.clone()),
        _ => None,
    })?;
    Some((text, t.position.epoch.0, t.position.seq.0))
}
