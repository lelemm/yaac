//! Attaching over Tailscale (docs/02-architecture.md#attaching-over-tailscale): with the
//! setting on, the hub listens on this host's Tailscale addresses (here a fake tailnet that
//! names loopback) and admits only connections from its own Tailscale login.

mod support;

use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use support::{BUILD, Client, Home, wait_until};
use yaac_hub::client::HubClient;
use yaac_hub::protocol::{StatusResult, TailscaleAttach, names as ext};
use yaac_hub::tailnet::{Tailnet, ThisNode};
use yaac_protocol::attach;
use yaac_transport::BoxFuture;

const ME: &str = "me@example.com";

/// A tailnet where this host is `127.0.0.1` on login [`ME`], and every peer has `peer`'s login
/// (`None`: Tailscale is not available).
#[derive(Clone)]
struct FakeTailnet {
    up: Arc<Mutex<bool>>,
    peer: Arc<Mutex<String>>,
}

impl Tailnet for FakeTailnet {
    fn this_node(&self) -> BoxFuture<'static, Result<ThisNode, String>> {
        let up = *self.up.lock().unwrap();
        Box::pin(async move {
            if up { Ok(ThisNode { addresses: vec![IpAddr::from([127, 0, 0, 1])], login: ME.into() }) } else { Err("Tailscale is not installed on this host.".into()) }
        })
    }

    fn login_of(&self, _peer: IpAddr) -> BoxFuture<'static, Result<String, String>> {
        let login = self.peer.lock().unwrap().clone();
        Box::pin(async move { Ok(login) })
    }
}

fn status(c: &Client) -> TailscaleAttach {
    let status: StatusResult = serde_json::from_value(c.call(ext::STATUS, json!({})).expect("status")).expect("status shape");
    status.tailscale
}

fn wait_for(c: &Client, what: &str, f: impl Fn(&TailscaleAttach) -> bool) -> TailscaleAttach {
    let mut last = TailscaleAttach::Off;
    wait_until(what, || {
        last = status(c);
        f(&last)
    });
    last
}

fn set(c: &Client, on: bool) -> Value {
    c.call(ext::SETTINGS_UPDATE, json!({ "tailscaleAttach": on })).expect("settings update")
}

#[test]
fn only_the_hosts_own_login_attaches() {
    let home = Home::new();
    let fake = FakeTailnet { up: Arc::new(Mutex::new(true)), peer: Arc::new(Mutex::new(ME.into())) };
    let mut config = home.config();
    config.tailnet = Arc::new(fake.clone());
    config.tailscale_port = 0;
    let hub = home.start_with(config);
    let local = hub.client();

    // Off by default: the hub listens only on its local socket.
    assert_eq!(status(&local), TailscaleAttach::Off);
    assert_eq!(local.call(ext::SETTINGS_GET, json!({})).unwrap()["settings"]["tailscaleAttach"], json!(false));

    let answer = set(&local, true);
    assert_eq!(answer["settings"]["tailscaleAttach"], json!(true));
    let TailscaleAttach::Listening { addresses, login } = wait_for(&local, "the Tailscale listener", |s| matches!(s, TailscaleAttach::Listening { .. })) else { unreachable!() };
    assert_eq!(login, ME);
    assert_eq!(addresses.len(), 1);
    let addr: SocketAddr = addresses[0].parse().expect("an address:port");
    assert!(addr.ip().is_loopback(), "bound only the tailnet's address, never a wildcard: {addr}");
    // The setting is persisted like the other hub settings.
    let file = std::fs::read_to_string(home.paths().settings_file()).unwrap();
    assert!(file.contains("\"tailscaleAttach\": true"), "{file}");

    // Same login: an ordinary attach — hello, then any method.
    let phone = Client { link: HubClient::connect_tcp(addr, || {}).expect("connect over TCP") };
    phone.call(attach::names::HELLO, json!({ "build": BUILD })).expect("hello over Tailscale");
    assert!(matches!(status(&phone), TailscaleAttach::Listening { .. }));

    // Logins are e-mail-like: case does not matter.
    *fake.peer.lock().unwrap() = ME.to_uppercase();
    let shouting = Client { link: HubClient::connect_tcp(addr, || {}).expect("connect over TCP") };
    shouting.call(attach::names::HELLO, json!({ "build": BUILD })).expect("hello with the login in another case");
    drop(shouting);

    // Another login: closed before any message.
    *fake.peer.lock().unwrap() = "someone-else@example.com".into();
    let stranger = HubClient::connect_tcp(addr, || {}).expect("the TCP connect itself succeeds");
    let _ = stranger.send(yaac_protocol::rpc::Message::request(stranger.next_id(), attach::names::HELLO, json!({ "build": BUILD })));
    wait_until("the hub closes the stranger's connection", || !stranger.is_connected());
    assert!(stranger.try_recv().is_none(), "the hub sent nothing to another login");

    // Turning the setting off stops listening and closes the phone's connection.
    set(&local, false);
    wait_for(&local, "the listener to stop", |s| *s == TailscaleAttach::Off);
    wait_until("the phone is detached", || !phone.link.is_connected());
    wait_until("the port is closed", || HubClient::connect_tcp(addr, || {}).is_err());

    hub.shutdown();
}

#[test]
fn a_missing_tailscale_is_a_status_not_a_crash() {
    let home = Home::new();
    let fake = FakeTailnet { up: Arc::new(Mutex::new(false)), peer: Arc::new(Mutex::new(ME.into())) };
    let mut config = home.config();
    config.tailnet = Arc::new(fake.clone());
    config.tailscale_port = 0;
    let hub = home.start_with(config);
    let local = hub.client();
    set(&local, true);
    let TailscaleAttach::Unavailable { reason } = wait_for(&local, "the unavailable status", |s| matches!(s, TailscaleAttach::Unavailable { .. })) else { unreachable!() };
    assert!(reason.contains("not installed"), "{reason}");
    // The window hears about changes without asking.
    *fake.up.lock().unwrap() = true;
    set(&local, false);
    set(&local, true);
    wait_for(&local, "listening once Tailscale is there", |s| matches!(s, TailscaleAttach::Listening { .. }));
    let notes: Vec<_> = local.drain().into_iter().filter(|m| matches!(m, yaac_protocol::rpc::Message::Notification { method, .. } if method == ext::TAILSCALE_STATUS)).collect();
    assert!(!notes.is_empty(), "hub/tailscale/status notifications reach the window");
    hub.shutdown();
}
