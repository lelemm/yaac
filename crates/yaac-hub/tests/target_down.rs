//! D35, D96: a target whose machine is not running (a stopped WSL distro) is reported as
//! `stopped`, with the notice text, and is never started behind the user's back: the hub keeps
//! checking without starting it, Retry (`hub/targets/reconnect`) does not start it either,
//! and **Start it** (`hub/targets/start`) is the one attempt allowed to — answered once the
//! target is connected, or with the reason it is not.

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use serde_json::json;
use support::{FakeDaemon, Home, wait_until};
use yaac_hub::ConnectorFactory;
use yaac_hub::protocol::names as ext;
use yaac_hub::settings::TargetDef;
use yaac_protocol::attach::ConnectionStatus;
use yaac_transport::{BoxFuture, ConnectCtx, ConnectError, Connector, Stream};

/// A fake daemon behind a "distro" that is stopped until someone connects with
/// `allow_start`.
struct Stopped {
    inner: FakeDaemon,
    checks: Arc<AtomicU32>,
    boot_fails: bool,
}

impl Connector for Stopped {
    fn connect<'a>(&'a self, ctx: &'a ConnectCtx) -> BoxFuture<'a, Result<Stream, ConnectError>> {
        Box::pin(async move {
            if !ctx.allow_start {
                self.checks.fetch_add(1, Ordering::SeqCst);
                return Err(ConnectError::TargetDown("WSL distro Ubuntu is not running".into()));
            }
            if self.boot_fails {
                return Err(ConnectError::Other("WSL distro Ubuntu failed to start: Wsl/Service/E_UNEXPECTED".into()));
            }
            self.inner.connect(ctx).await
        })
    }

    fn describe(&self) -> String {
        "stopped distro".into()
    }
}

struct Factory {
    inner: FakeDaemon,
    checks: Arc<AtomicU32>,
    boot_fails: bool,
}

impl ConnectorFactory for Factory {
    fn connector(&self, _target: &TargetDef) -> Result<Arc<dyn Connector>, String> {
        Ok(Arc::new(Stopped { inner: self.inner.clone(), checks: self.checks.clone(), boot_fails: self.boot_fails }))
    }
}

#[test]
fn a_down_target_is_shown_down_and_starts_only_when_asked() {
    let home = Home::new();
    let checks = Arc::new(AtomicU32::new(0));
    let fake = FakeDaemon::new("wsl");
    let mut config = home.config();
    config.connectors = Arc::new(Factory { inner: fake.clone(), checks: checks.clone(), boot_fails: false });
    let hub = home.start_with(config);
    let c = hub.client();
    let target = c.add_target("wsl").id;

    wait_until("shown as down", || {
        c.targets().iter().any(|t| {
            t.id == target && t.enabled && t.state.status == ConnectionStatus::Stopped && t.state.detail.as_deref() == Some("WSL distro Ubuntu is not running")
        })
    });
    // It keeps checking (which starts nothing) and stays down, never flickering to connecting.
    let before = checks.load(Ordering::SeqCst);
    wait_until("checked again", || checks.load(Ordering::SeqCst) > before + 1);
    let t = c.targets().into_iter().find(|t| t.id == target).unwrap();
    assert_eq!(t.state.status, ConnectionStatus::Stopped);
    // Retry is not Start it.
    let before = checks.load(Ordering::SeqCst);
    c.call(ext::TARGETS_RECONNECT, json!({ "id": target })).unwrap();
    wait_until("checked again after retry", || checks.load(Ordering::SeqCst) > before);
    assert!(!fake.connected(), "never started without Start it");

    // Start it: answered once connected; the target is back to normal with no further action.
    c.call(ext::TARGETS_START, json!({ "id": target })).expect("start it");
    let t = c.targets().into_iter().find(|t| t.id == target).unwrap();
    assert_eq!(t.state.status, ConnectionStatus::Connected, "connected when the answer arrives");
    assert!(fake.connected());
    // Already connected: answered at once.
    c.call(ext::TARGETS_START, json!({ "id": target })).expect("start it again");
    drop(c);
    std::thread::sleep(Duration::from_millis(20));
    hub.shutdown();
}

#[test]
fn start_it_reports_a_readable_failure() {
    let home = Home::new();
    let checks = Arc::new(AtomicU32::new(0));
    let fake = FakeDaemon::new("wsl");
    let mut config = home.config();
    config.connectors = Arc::new(Factory { inner: fake.clone(), checks, boot_fails: true });
    let hub = home.start_with(config);
    let c = hub.client();
    let target = c.add_target("wsl").id;
    c.wait_status(&target, ConnectionStatus::Stopped);
    let e = c.call(ext::TARGETS_START, json!({ "id": target })).unwrap_err();
    assert!(e.message.contains("failed to start: Wsl/Service/E_UNEXPECTED"), "{e:?}");
    assert!(!fake.connected());
    drop(c);
    hub.shutdown();
}
