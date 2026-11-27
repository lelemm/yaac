//! Moving the hub to another machine (docs/15-hub-transfer.md): `hub/transfer/candidates` and
//! `hub/transfer/start` against fake daemons. The release lookup and start-at-login are a fake
//! [`TransferHost`]; nothing is downloaded and the real autostart entry is never touched.

mod support;

use std::net::IpAddr;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use support::{Client, FakeDaemon, Home, RunningHub, wait_until};
use yaac_hub::client::HubClient;
use yaac_hub::protocol::names as ext;
use yaac_hub::settings::Settings;
use yaac_hub::tailnet::{Tailnet, ThisNode};
use yaac_hub::transfer::{HubAsset, TransferHost, UNREACHABLE_FIELD};
use yaac_hub::{ExitReason, HubConfig, HubError};
use yaac_protocol::attach::{self, HubMoved, HubTransferCandidatesResult, HubTransferProgress, TargetEntry, TargetTransport, hub_transfer};
use yaac_protocol::methods::names;
use yaac_protocol::rpc::Message;
use yaac_protocol::{Capability, TargetId};
use yaac_transport::BoxFuture;

const ME: &str = "me@example.com";
/// A release build: a development build cannot move.
const HUB: &str = "0.1.0+1050";

struct FakeTailnet;

impl Tailnet for FakeTailnet {
    fn this_node(&self) -> BoxFuture<'static, Result<ThisNode, String>> {
        Box::pin(async { Ok(ThisNode { addresses: vec![IpAddr::from([127, 0, 0, 1])], login: ME.into() }) })
    }
    fn login_of(&self, _peer: IpAddr) -> BoxFuture<'static, Result<String, String>> {
        Box::pin(async { Ok(ME.to_owned()) })
    }
}

#[derive(Default)]
struct FakeHost {
    calls: Mutex<Vec<String>>,
}

const ASSET_SHA: &str = "abababababababababababababababababababababababababababababababab";

impl TransferHost for FakeHost {
    fn hub_asset(&self, build: &str, arch: &str) -> Result<HubAsset, String> {
        self.calls.lock().unwrap().push(format!("asset {build} {arch}"));
        Ok(HubAsset { url: format!("https://example.invalid/{}", yaac_hub::transfer::hub_asset_name(arch)), sha256: ASSET_SHA.into(), size: 1234 })
    }
    fn stop_autostart(&self) -> Result<(), String> {
        self.calls.lock().unwrap().push("autostart off".into());
        Ok(())
    }
}

struct Setup {
    home: Home,
    host: Arc<FakeHost>,
    x: FakeDaemon,
    y: FakeDaemon,
    here: FakeDaemon,
}

fn setup() -> Setup {
    let home = Home::new();
    let x = FakeDaemon::new("x");
    x.set_features(vec![Capability::HubHost]);
    // An older yaacd: no hubHost.
    let y = FakeDaemon::new("y");
    y.set_features(vec![]);
    let here = FakeDaemon::new("here");
    here.set_features(vec![Capability::HubHost]);
    for d in [&x, &y, &here] {
        home.fakes.add(d);
    }
    x.hub_host(|h| h.watch = vec![y.clone(), here.clone()]);
    Setup { home, host: Arc::new(FakeHost::default()), x, y, here }
}

fn config(s: &Setup, build: &str) -> HubConfig {
    let mut config = s.home.config();
    config.build = build.into();
    config.tailnet = Arc::new(FakeTailnet);
    config.tailscale_port = 0;
    config.transfer_host = Some(s.host.clone());
    config
}

fn client(hub: &RunningHub, build: &str) -> Client {
    let c = Client { link: HubClient::connect(&hub.socket).unwrap() };
    c.call(attach::names::HELLO, json!({ "build": build })).expect("hello");
    c
}

fn add(c: &Client, name: &str, transport: &Value) -> TargetEntry {
    let r = c.call(attach::names::TARGETS_ADD, json!({ "name": name, "transport": transport, "enabled": true })).expect("add");
    serde_json::from_value(r["target"].clone()).unwrap()
}

/// x and y over SSH, here local, w a WSL distro.
fn targets(c: &Client) -> (TargetId, TargetId, TargetId, TargetId) {
    let x = add(c, "x", &json!({ "kind": "ssh", "host": "x.example" })).id;
    let y = add(c, "y", &json!({ "kind": "ssh", "host": "y.example" })).id;
    let here = add(c, "here", &json!({ "kind": "local" })).id;
    let w = add(c, "w", &json!({ "kind": "wsl", "distro": "Ubuntu" })).id;
    for id in [&x, &y, &here] {
        c.wait_status(id, attach::ConnectionStatus::Connected);
    }
    (x, y, here, w)
}

fn candidates(c: &Client) -> HubTransferCandidatesResult {
    serde_json::from_value(c.call(attach::names::TRANSFER_CANDIDATES, json!({})).expect("candidates")).unwrap()
}

fn reason(r: &HubTransferCandidatesResult, id: &TargetId) -> Option<String> {
    let c = r.candidates.iter().find(|c| &c.target_id == id).expect("every target is listed");
    assert_eq!(c.eligible, c.reason.is_none());
    c.reason.clone()
}

/// Notifications received so far, kept across calls.
#[derive(Default)]
struct Heard {
    progress: Vec<HubTransferProgress>,
    moved: Vec<HubMoved>,
}

impl Heard {
    fn take(&mut self, c: &Client) {
        for m in c.drain() {
            if let Message::Notification { method, params } = m {
                if method == attach::names::TRANSFER_PROGRESS {
                    self.progress.push(serde_json::from_value(params).unwrap());
                } else if method == attach::names::MOVED {
                    self.moved.push(serde_json::from_value(params).unwrap());
                }
            }
        }
    }
    fn phases(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for p in &self.progress {
            if out.last() != Some(&p.phase.as_str()) {
                out.push(&p.phase);
            }
        }
        out
    }
}

fn calls(d: &FakeDaemon) -> Vec<String> {
    d.hub_host(|h| h.calls.iter().map(|(m, _)| m.clone()).collect())
}

#[test]
fn every_target_is_listed_with_why_it_cannot_take_the_hub() {
    let s = setup();
    let hub = s.home.start_with(config(&s, HUB));
    let c = client(&hub, HUB);
    let (x, y, here, w) = targets(&c);

    let r = candidates(&c);
    assert_eq!(reason(&r, &x), None, "Linux, same Tailscale login, port free, no hub there");
    assert_eq!(reason(&r, &y).as_deref(), Some("Update YAAC on that target first."));
    assert_eq!(reason(&r, &here).as_deref(), Some("This is the hub's own computer."));
    assert!(reason(&r, &w).unwrap().contains("WSL"), "WSL shares Windows' Tailscale address");
    // Only x and here have hubHost; only x was asked (here is ruled out by its transport).
    assert_eq!(calls(&s.x), [names::HUB_HOST_PROBE]);
    assert!(calls(&s.here).is_empty());

    // What the probe says decides.
    s.x.hub_host(|h| {
        let mut p = support::fake_daemon::default_probe();
        p["tailscale"]["login"] = json!("someone@else.com");
        h.probe = Some(p);
    });
    assert!(reason(&candidates(&c), &x).unwrap().contains("another Tailscale login (someone@else.com)"));
    s.x.hub_host(|h| {
        let mut p = support::fake_daemon::default_probe();
        p["hubRunning"] = json!(true);
        h.probe = Some(p);
    });
    assert!(reason(&candidates(&c), &x).unwrap().contains("already runs"));

    // A disconnected target: connect it first.
    s.x.set_offline(true);
    c.wait_status(&x, attach::ConnectionStatus::Offline);
    assert_eq!(reason(&candidates(&c), &x).as_deref(), Some("Connect it first."));
    // Asking to move there anyway is refused at once.
    let e = c.call(attach::names::TRANSFER_START, json!({ "opId": "op-1", "targetId": x })).unwrap_err();
    assert_eq!(e.code.as_str(), hub_transfer::codes::NOT_ELIGIBLE);
    hub.shutdown();
}

#[test]
fn a_development_build_cannot_move() {
    let s = setup();
    let dev = support::BUILD;
    let hub = s.home.start_with(config(&s, dev));
    let c = client(&hub, dev);
    let (x, ..) = targets(&c);
    assert!(reason(&candidates(&c), &x).unwrap().contains("development build"));
    hub.shutdown();
}

#[test]
fn the_hub_moves_and_lets_go_of_every_target_first() {
    let s = setup();
    let hub = s.home.start_with(config(&s, HUB));
    let c = client(&hub, HUB);
    let (x, y, here, _w) = targets(&c);
    c.call(ext::DRAFTS_PUT, json!({ "key": "k1", "target": here, "threadId": "t1", "text": "half a thought" })).unwrap();
    c.call(ext::UI_STATE_SET, json!({ "key": "collapsed", "value": ["g1"] })).unwrap();
    s.x.hub_host(|h| h.hold_start = true);
    let hub_id = serde_json::from_value::<Value>(c.call(attach::names::HELLO, json!({ "build": HUB })).unwrap()).unwrap()["hubId"].clone();

    // Bad op ids never reach the target.
    assert!(c.call(attach::names::TRANSFER_START, json!({ "opId": "../x", "targetId": x })).is_err());
    assert_eq!(c.call(attach::names::TRANSFER_START, json!({ "opId": "op-1", "targetId": x })).unwrap(), json!({}));
    // One at a time.
    let e = c.call(attach::names::TRANSFER_START, json!({ "opId": "op-2", "targetId": x })).unwrap_err();
    assert_eq!(e.code.as_str(), hub_transfer::codes::IN_PROGRESS);

    let mut heard = Heard::default();
    wait_until("the arming", || calls(&s.x).contains(&names::HUB_HOST_START.to_owned()));
    // Handing over: nothing that changes state is taken, reads still are.
    let e = c.call(ext::UI_STATE_SET, json!({ "key": "collapsed", "value": [] })).unwrap_err();
    assert_eq!(e.code.as_str(), hub_transfer::codes::MOVING);
    let e = c.call(attach::names::SHUTDOWN, json!({})).unwrap_err();
    assert_eq!(e.code.as_str(), hub_transfer::codes::MOVING);
    c.call(attach::names::TARGETS_LIST, json!({})).expect("reads are served");
    // Every other target was let go before the new hub was armed; x stays.
    assert_eq!(s.x.hub_host(|h| h.connected_at_start.clone()), [("y".to_owned(), false), ("here".to_owned(), false)]);
    assert!(s.x.connected());

    let sequence = calls(&s.x);
    assert_eq!(&sequence[..2], [names::HUB_HOST_PROBE, names::HUB_HOST_INSTALL]);
    assert_eq!(sequence.last().unwrap(), names::HUB_HOST_START);
    assert!(sequence[2..sequence.len() - 1].iter().all(|m| m == names::HUB_HOST_PUT), "{sequence:?}");
    let params = s.x.hub_host(|h| h.calls.clone());
    assert_eq!(params[1].1, json!({ "opId": "op-1", "url": "https://example.invalid/yaac-hub-linux-x86_64", "sha256": ASSET_SHA, "size": 1234 }));
    assert_eq!(params.last().unwrap().1, json!({ "opId": "op-1", "armOnClose": true }));
    assert_eq!(*s.host.calls.lock().unwrap(), [format!("asset {HUB} x86_64")]);

    // The files arrived whole (the fake checked each SHA-256), the live cache first.
    let staged = s.x.hub_host(|h| h.staged.clone());
    let names: Vec<&str> = staged.iter().map(|(n, _)| n.as_str()).collect();
    let puts: Vec<String> = params.iter().filter(|(m, _)| m == names::HUB_HOST_PUT).map(|(_, p)| p["name"].as_str().unwrap().to_owned()).collect();
    assert_eq!(puts[0], "data/cache.sqlite");
    assert_eq!(&puts[1..3], ["data/relay.sqlite", "data/drafts.sqlite"]);
    assert_eq!(puts.last().unwrap(), "config/hub.json");
    for name in ["data/cache.sqlite", "data/relay.sqlite", "data/drafts.sqlite", "config/hub.json"] {
        assert!(names.contains(&name), "{name} staged: {names:?}");
    }
    for (name, bytes) in &staged {
        if name.ends_with(".sqlite") {
            assert!(bytes.starts_with(b"SQLite format 3\0"), "{name} is a database");
        }
    }
    // The rewritten settings: x is the new hub's own target, same id; here is not reachable.
    let hub_json = &staged.iter().find(|(n, _)| n == "config/hub.json").unwrap().1;
    let moved: Settings = serde_json::from_slice(hub_json).unwrap();
    assert!(matches!(moved.target(&x).unwrap().transport, TargetTransport::Local(_)));
    assert!(!moved.target(&here).unwrap().enabled);
    assert!(moved.target(&here).unwrap().extra.contains_key(UNREACHABLE_FIELD));
    assert!(moved.target(&y).unwrap().enabled, "an SSH target stays");
    assert!(moved.hub.tailscale_attach, "every client is remote now");
    assert_eq!(json!(moved.hub_id), hub_id);
    // The draft made before moving went with it.
    let drafts = staged.iter().find(|(n, _)| n == "data/drafts.sqlite").unwrap().1.clone();
    let file = s.home.dir.path().join("drafts-copy.sqlite");
    std::fs::write(&file, drafts).unwrap();
    let copy = yaac_hub::drafts::DraftStore::open(&file).unwrap();
    assert_eq!(copy.list().unwrap().len(), 1);

    // Armed: the tombstone, every client told, x closed last, the hub gone.
    s.x.release_start();
    let mut hub = hub;
    let exit = hub.join();
    assert_eq!(exit.reason, ExitReason::Moved);
    heard.take(&c);
    assert_eq!(heard.phases(), [hub_transfer::CHECKING, hub_transfer::INSTALLING, hub_transfer::COPYING, hub_transfer::HANDING_OVER, hub_transfer::MOVED]);
    assert!(heard.progress.iter().all(|p| p.op_id.as_str() == "op-1" && p.target_id == x));
    assert_eq!(heard.moved.len(), 1);
    assert_eq!(heard.moved[0].address, "100.64.0.7:47380");
    assert_eq!(heard.moved[0].host_name, "x");
    assert_eq!(json!(heard.moved[0].hub_id), hub_id);
    let tomb = yaac_hub::paths::moved_tombstone(&s.home.paths()).unwrap().expect("tombstone");
    assert_eq!((tomb.address.as_str(), tomb.host_name.as_str()), ("100.64.0.7:47380", "x"));
    assert_eq!(s.host.calls.lock().unwrap().last().map(String::as_str), Some("autostart off"));
    wait_until("x closed", || !s.x.connected());
    assert!(!s.y.connected() && !s.here.connected());
    assert!(!calls(&s.x).contains(&names::HUB_HOST_ABORT.to_owned()));
    assert!(!s.home.paths().data_dir.join("transfer-op-1").exists(), "the local staging is gone");

    // The hub does not start here again until the tombstone is removed.
    match yaac_hub::run_hub(config(&s, HUB)) {
        Err(HubError::Moved(t)) => assert_eq!(t, tomb),
        other => panic!("started with a tombstone: {other:?}"),
    }
    assert!(yaac_hub::paths::remove_moved_tombstone(&s.home.paths()).unwrap());
    let hub = s.home.start_with(config(&s, HUB));
    hub.shutdown();
}

#[test]
fn a_failure_before_the_hand_over_moves_nothing() {
    let s = setup();
    let hub = s.home.start_with(config(&s, HUB));
    let c = client(&hub, HUB);
    let (x, _y, here, _w) = targets(&c);
    s.x.hub_host(|h| h.fail_put = Some(("data/relay.sqlite".into(), "No space left on device".into())));

    c.call(attach::names::TRANSFER_START, json!({ "opId": "op-1", "targetId": x })).unwrap();
    let mut heard = Heard::default();
    wait_until("the failure", || {
        heard.take(&c);
        heard.progress.iter().any(|p| p.phase == hub_transfer::FAILED)
    });
    let failed = heard.progress.iter().find(|p| p.phase == hub_transfer::FAILED).unwrap();
    assert!(failed.message.as_deref().unwrap().contains("No space left on device"), "{failed:?}");
    assert!(heard.phases().contains(&hub_transfer::HANDING_OVER), "it failed while handing over");
    // Staging discarded on x, the other targets back, no tombstone, start-at-login untouched.
    wait_until("the abort", || calls(&s.x).last().map(String::as_str) == Some(names::HUB_HOST_ABORT));
    assert_eq!(s.x.hub_host(|h| h.calls.last().unwrap().1.clone()), json!({ "opId": "op-1" }));
    assert!(!calls(&s.x).contains(&names::HUB_HOST_START.to_owned()));
    c.wait_status(&here, attach::ConnectionStatus::Connected);
    wait_until("here reconnected", || s.here.connected());
    assert!(s.x.connected(), "x was never let go");
    assert!(yaac_hub::paths::moved_tombstone(&s.home.paths()).unwrap().is_none());
    assert!(!s.host.calls.lock().unwrap().iter().any(|c| c == "autostart off"));
    assert!(heard.moved.is_empty());
    // The hub carries on, and never tries again by itself.
    c.call(ext::UI_STATE_SET, json!({ "key": "collapsed", "value": [] })).expect("state changes are taken again");
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert_eq!(calls(&s.x).iter().filter(|m| *m == names::HUB_HOST_PROBE).count(), 1);
    wait_until("the local staging removed", || !s.home.paths().data_dir.join("transfer-op-1").exists());
    hub.shutdown();
}
