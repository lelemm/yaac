//! `hub/update/request` (frozen; docs/install.md#updating-from-the-phone): a phone newer than
//! the hub asks for its build to be installed on the hub's computer. Accepted despite the
//! build mismatch, only over Tailscale, only for a newer published build, once per request.
//! The installer here is a fake: nothing is downloaded or run.

mod support;

use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use support::{Client, Home, wait_until};
use yaac_hub::client::HubClient;
use yaac_hub::host_update::{HostInstaller, HostRelease};
use yaac_hub::protocol::{StatusResult, TailscaleAttach, names as ext};
use yaac_hub::tailnet::{Tailnet, ThisNode};
use yaac_protocol::attach::{self, HostUpdateProgress, host_update};
use yaac_protocol::rpc::Message;
use yaac_transport::BoxFuture;

const ME: &str = "me@example.com";
/// The hub's build: a release build, so newer ones can be asked for.
const HUB: &str = "0.1.0+1050";
const PHONE: &str = "0.1.0+1060";

struct FakeTailnet;

impl Tailnet for FakeTailnet {
    fn this_node(&self) -> BoxFuture<'static, Result<ThisNode, String>> {
        Box::pin(async { Ok(ThisNode { addresses: vec![IpAddr::from([127, 0, 0, 1])], login: ME.into() }) })
    }
    fn login_of(&self, _peer: IpAddr) -> BoxFuture<'static, Result<String, String>> {
        Box::pin(async { Ok(ME.to_owned()) })
    }
}

/// Publishes only [`PHONE`]; records every step it was asked to do.
#[derive(Default)]
struct FakeInstaller {
    calls: Mutex<Vec<String>>,
    /// The download fails with this.
    fail_download: Mutex<Option<String>>,
}

impl HostInstaller for FakeInstaller {
    fn find(&self, build: &str) -> Result<HostRelease, String> {
        self.calls.lock().unwrap().push(format!("find {build}"));
        if build == PHONE { Ok(HostRelease { build: build.into(), size: 1000, detail: Box::new(()) }) } else { Err(format!("There is no published release of YAAC {build}.")) }
    }
    fn download(&self, release: &HostRelease, progress: &mut dyn FnMut(u64, u64)) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("download {}", release.build));
        if let Some(e) = self.fail_download.lock().unwrap().take() {
            return Err(e);
        }
        progress(0, 1000);
        progress(1000, 1000);
        Ok(())
    }
    fn install(&self, release: &HostRelease) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("install {}", release.build));
        Ok(())
    }
}

fn listen(local: &Client) -> SocketAddr {
    local.call(ext::SETTINGS_UPDATE, json!({ "tailscaleAttach": true })).expect("settings update");
    let mut addr = None;
    wait_until("the Tailscale listener", || {
        let status: StatusResult = serde_json::from_value(local.call(ext::STATUS, json!({})).unwrap()).unwrap();
        if let TailscaleAttach::Listening { addresses, .. } = status.tailscale {
            addr = addresses.first().and_then(|a| a.parse().ok());
        }
        addr.is_some()
    });
    addr.unwrap()
}

fn hello(link: HubClient, build: &str) -> (Client, Result<Value, yaac_protocol::rpc::ErrorObject>) {
    let c = Client { link };
    let r = c.call(attach::names::HELLO, json!({ "build": build }));
    (c, r)
}

fn request(c: &Client, build: &str, op: &str) -> Result<Value, yaac_protocol::rpc::ErrorObject> {
    c.call(attach::names::UPDATE_REQUEST, json!({ "build": build, "opId": op }))
}

fn progress(c: &Client) -> Vec<HostUpdateProgress> {
    c.drain().into_iter().filter_map(|m| match m {
        Message::Notification { method, params } if method == attach::names::UPDATE_PROGRESS => serde_json::from_value(params).ok(),
        _ => None,
    }).collect()
}

fn code(r: Result<Value, yaac_protocol::rpc::ErrorObject>) -> String {
    r.expect_err("refused").code.as_str().to_owned()
}

#[test]
fn a_newer_phone_updates_the_pc_once_and_nothing_else_gets_through() {
    let home = Home::new();
    let installer = Arc::new(FakeInstaller::default());
    let mut config = home.config();
    config.build = HUB.into();
    config.tailnet = Arc::new(FakeTailnet);
    config.tailscale_port = 0;
    config.host_installer = Some(installer.clone());
    let hub = home.start_with(config);
    // The window on the PC: the hub's own build.
    let (window, hi) = hello(HubClient::connect(&hub.socket).unwrap(), HUB);
    hi.expect("the window attaches");
    let addr = listen(&window);

    // A window on this computer never asks for this: it updates itself.
    assert_eq!(code(request(&window, PHONE, "op-local")), host_update::codes::NOT_ALLOWED);

    // The phone is newer: its hello is accepted, every ordinary method refused…
    let (phone, hi) = hello(HubClient::connect_tcp(addr, || {}).unwrap(), PHONE);
    assert_eq!(hi.expect("a newer client's hello is answered")["build"], HUB);
    assert!(phone.call(attach::names::TARGETS_LIST, json!({})).is_err(), "mismatched: nothing else is served");
    // …and the update request is not, but never for the same build, an older one, or a non-release.
    for (build, why) in [(HUB, "the same build"), ("0.1.0+1049", "a downgrade"), ("0.1.0+dev.ab", "a development build"), ("0.1.0/x+1070", "a tag that is not one")] {
        assert_eq!(code(request(&phone, build, "op-bad")), host_update::codes::NOT_NEWER, "{why}");
    }
    // Newer, but never published: refused after asking, nothing downloaded.
    assert_eq!(code(request(&phone, "0.1.0+1070", "op-unknown")), host_update::codes::UNAVAILABLE);
    assert_eq!(*installer.calls.lock().unwrap(), ["find 0.1.0+1070"]);

    // A download that fails is reported, and not tried again by the hub.
    *installer.fail_download.lock().unwrap() = Some("the download does not match the release's SHA-256".into());
    let r = request(&phone, PHONE, "op-1").expect("accepted");
    assert_eq!(r, json!({ "build": PHONE, "size": 1000 }));
    let mut seen = Vec::new();
    wait_until("the failure", || {
        seen.extend(progress(&phone));
        seen.iter().any(|p| p.phase == host_update::FAILED)
    });
    assert!(seen.last().unwrap().message.as_deref().unwrap().contains("SHA-256"));
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert_eq!(*installer.calls.lock().unwrap(), ["find 0.1.0+1070", "find 0.1.0+1060", "download 0.1.0+1060"], "one request, one attempt");

    // The next tap is a new request: downloaded, verified, the installer started once.
    progress(&window);
    request(&phone, PHONE, "op-2").expect("accepted");
    let mut seen = Vec::new();
    wait_until("installing", || {
        seen.extend(progress(&phone));
        seen.iter().any(|p| p.phase == host_update::INSTALLING)
    });
    let phases: Vec<&str> = seen.iter().map(|p| p.phase.as_str()).collect();
    assert_eq!(phases.first(), Some(&host_update::CHECKING));
    assert!(phases.contains(&host_update::DOWNLOADING), "{phases:?}");
    assert!(seen.iter().all(|p| p.op_id.as_str() == "op-2" && p.build == PHONE));
    assert_eq!(installer.calls.lock().unwrap().last().map(String::as_str), Some("install 0.1.0+1060"));
    // The window on the PC heard it too, before the installer closes it.
    let mut heard = Vec::new();
    wait_until("the window's notice", || {
        heard.extend(progress(&window));
        heard.iter().any(|p| p.phase == host_update::INSTALLING)
    });
    // Once the installer runs, nothing else is taken.
    assert_eq!(code(request(&phone, PHONE, "op-3")), host_update::codes::IN_PROGRESS);
    assert_eq!(installer.calls.lock().unwrap().iter().filter(|c| c.starts_with("install")).count(), 1);
    hub.shutdown();
}

#[test]
fn a_host_without_an_installer_says_so() {
    let home = Home::new();
    let mut config = home.config();
    config.build = HUB.into();
    config.tailnet = Arc::new(FakeTailnet);
    config.tailscale_port = 0;
    let hub = home.start_with(config);
    let (window, hi) = hello(HubClient::connect(&hub.socket).unwrap(), HUB);
    hi.unwrap();
    let addr = listen(&window);
    let phone = Client { link: HubClient::connect_tcp(addr, || {}).unwrap() };
    // Before hello: nothing.
    assert_eq!(code(request(&phone, PHONE, "op-0")), "hello_required");
    phone.call(attach::names::HELLO, json!({ "build": PHONE })).unwrap();
    let e = request(&phone, PHONE, "op-1").unwrap_err();
    assert_eq!(e.code.as_str(), host_update::codes::UNSUPPORTED);
    assert!(e.message.contains("not supported on this host"), "{}", e.message);
    hub.shutdown();
}

#[test]
fn a_phone_never_stops_the_hub_or_changes_when_it_runs() {
    let home = Home::new();
    let mut config = home.config();
    config.build = HUB.into();
    config.tailnet = Arc::new(FakeTailnet);
    config.tailscale_port = 0;
    let hub = home.start_with(config);
    let (window, hi) = hello(HubClient::connect(&hub.socket).unwrap(), HUB);
    hi.unwrap();
    let addr = listen(&window);
    // Newer (where a local window would be told to send hub/shutdown), the same build, older:
    // whatever the builds, the phone cannot stop the hub.
    for build in [PHONE, HUB, "0.1.0+1040"] {
        let phone = Client { link: HubClient::connect_tcp(addr, || {}).unwrap() };
        let _ = phone.call(attach::names::HELLO, json!({ "build": build }));
        assert_eq!(code(phone.call(attach::names::SHUTDOWN, json!({}))), "remote_not_allowed", "phone on {build}");
    }
    // Attached on the same build: start at login and the idle exit are the PC's; its own
    // access (tailscaleAttach) and everything else stay the phone's to change.
    let (phone, hi) = hello(HubClient::connect_tcp(addr, || {}).unwrap(), HUB);
    hi.expect("same build");
    assert_eq!(code(phone.call(ext::SETTINGS_UPDATE, json!({ "always": false }))), "remote_not_allowed");
    assert_eq!(code(phone.call(ext::SETTINGS_UPDATE, json!({ "idleExitSecs": 1 }))), "remote_not_allowed");
    let settings = phone.call(ext::SETTINGS_GET, json!({})).unwrap();
    assert_eq!(settings["settings"]["always"], json!(true), "nothing was written");
    phone.call(ext::SETTINGS_UPDATE, json!({ "notifications": settings["settings"]["notifications"] })).expect("other settings are the phone's too");
    assert!(phone.call(attach::names::TARGETS_LIST, json!({})).is_ok());
    // The window on the PC still may.
    window.call(ext::SETTINGS_UPDATE, json!({ "always": true })).expect("the PC's window sets it");
    hub.shutdown(); // a local client: allowed
}
