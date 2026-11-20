//! D35: a target whose machine is down (a stopped WSL distro) is shown as down — disabled,
//! with the notice text — and is never started behind the user's back: the hub keeps
//! checking without starting it, and connects when the user asks (`hub/targets/reconnect`,
//! "Start it"), which is the one attempt allowed to start it.

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
}

impl Connector for Stopped {
    fn connect<'a>(&'a self, ctx: &'a ConnectCtx) -> BoxFuture<'a, Result<Stream, ConnectError>> {
        Box::pin(async move {
            if !ctx.allow_start {
                self.checks.fetch_add(1, Ordering::SeqCst);
                return Err(ConnectError::TargetDown("WSL distro Ubuntu is not running".into()));
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
}

impl ConnectorFactory for Factory {
    fn connector(&self, _target: &TargetDef) -> Result<Arc<dyn Connector>, String> {
        Ok(Arc::new(Stopped { inner: self.inner.clone(), checks: self.checks.clone() }))
    }
}

#[test]
fn a_down_target_is_shown_down_and_starts_only_when_asked() {
    let home = Home::new();
    let checks = Arc::new(AtomicU32::new(0));
    let fake = FakeDaemon::new("wsl");
    let mut config = home.config();
    config.connectors = Arc::new(Factory { inner: fake.clone(), checks: checks.clone() });
    let hub = home.start_with(config);
    let c = hub.client();
    let target = c.add_target("wsl").id;

    wait_until("shown as down", || {
        c.targets().iter().any(|t| {
            t.id == target && t.enabled && t.state.status == ConnectionStatus::Disabled && t.state.detail.as_deref() == Some("WSL distro Ubuntu is not running")
        })
    });
    // It keeps checking (which starts nothing) and stays down, never flickering to connecting.
    let before = checks.load(Ordering::SeqCst);
    wait_until("checked again", || checks.load(Ordering::SeqCst) > before + 1);
    let t = c.targets().into_iter().find(|t| t.id == target).unwrap();
    assert_eq!(t.state.status, ConnectionStatus::Disabled);
    assert!(!fake.connected(), "never started without being asked");

    // "Start it".
    c.call(ext::TARGETS_RECONNECT, json!({ "id": target })).unwrap();
    c.wait_status(&target, ConnectionStatus::Connected);
    assert!(fake.connected());
    drop(c);
    std::thread::sleep(Duration::from_millis(20));
    hub.shutdown();
}
