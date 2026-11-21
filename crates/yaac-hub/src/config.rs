//! How a hub is configured: paths, build, connectors, and timings.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use yaac_protocol::attach::TargetTransport;
use yaac_transport::{Connector, LocalConfig, LocalConnector};

use crate::notify::{Notifier, SystemNotifier};
use crate::paths::{HubPaths, PathsError};
use crate::settings::TargetDef;

/// Makes the connector for a target. The default builds a [`LocalConnector`] for local
/// targets and refuses SSH and WSL (a later package); tests supply in-memory daemons.
pub trait ConnectorFactory: Send + Sync {
    /// The connector for this target, or a message to show as the target's state.
    fn connector(&self, target: &TargetDef) -> Result<Arc<dyn Connector>, String>;
}

/// The default connector factory.
#[derive(Clone, Debug)]
pub struct DefaultConnectors {
    /// How the local runtime and bundle are found.
    pub local: LocalConfig,
}

impl ConnectorFactory for DefaultConnectors {
    fn connector(&self, target: &TargetDef) -> Result<Arc<dyn Connector>, String> {
        match &target.transport {
            TargetTransport::Local(_) => Ok(Arc::new(LocalConnector::new(self.local.clone()))),
            TargetTransport::Ssh(_) => Err("SSH targets are not supported by this build yet".into()),
            TargetTransport::Wsl(_) => Err("WSL targets are not supported by this build yet".into()),
            TargetTransport::Unknown(other) => Err(format!("unknown transport {:?}", other.tag("kind"))),
        }
    }
}

/// Reconnection backoff: capped exponential.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Backoff {
    /// First delay.
    pub initial: Duration,
    /// Cap.
    pub max: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Self { initial: Duration::from_millis(500), max: Duration::from_secs(30) }
    }
}

/// Everything `run_hub` needs.
#[derive(Clone)]
pub struct HubConfig {
    /// This build (`hub/hello` compares it with the window's).
    pub build: String,
    /// Where files go.
    pub paths: HubPaths,
    /// Makes each target's connector.
    pub connectors: Arc<dyn ConnectorFactory>,
    /// Overrides the idle period from the settings file (tests).
    pub idle_exit: Option<Duration>,
    /// Reconnection backoff.
    pub backoff: Backoff,
    /// How often committed positions are acknowledged (`thread/ack`).
    pub ack_interval: Duration,
    /// How often an idle connection is pinged.
    pub ping_interval: Duration,
    /// A connection that has sent nothing for this long is dropped and reconnected.
    pub dead_after: Duration,
    /// Cap on cached transcript bytes (least-recently-used threads are evicted beyond it).
    pub cache_cap_bytes: u64,
    /// Cap on the bytes read when a thread's transcript is rebuilt after a `reset`.
    pub rebuild_cap_bytes: u64,
    /// On first run, add an enabled local target named `local`.
    pub seed_local_target: bool,
    /// On first run, the `always` setting written to `hub.json` (default `true`: the hub
    /// runs always and starts at login; `false` is the idle-exit mode).
    pub first_run_always: bool,
    /// Serve the `hub/debug/*` methods (tests only).
    pub test_methods: bool,
    /// This host's name, sent in `initialize`.
    pub host_name: String,
    /// Stop cleanly on SIGINT / SIGTERM (off in tests that run several hubs in one process).
    pub handle_signals: bool,
    /// A relayed message undelivered this long earns its sender a passive notice
    /// (docs/10-agent-mesh.md#local-limits: 15 minutes).
    pub undelivered_notice: Duration,
    /// How often the relay is swept for such messages.
    pub notice_sweep: Duration,
    /// Desktop notifications when a thread needs the user and no window is attached. `None`:
    /// off (the default of [`HubConfig::new`], and so of tests).
    pub notifier: Option<Arc<dyn Notifier>>,
    /// Upload limits (docs/06-attachments.md).
    pub uploads: crate::UploadLimits,
    /// Threads in the worker pool that reads and hashes files for uploads.
    pub pool_threads: usize,
}

impl std::fmt::Debug for HubConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HubConfig").field("build", &self.build).field("paths", &self.paths).finish_non_exhaustive()
    }
}

/// The repository root used to find the daemon bundle during development: `YAAC_ROOT`, else
/// the current directory.
pub fn default_root() -> PathBuf {
    std::env::var_os("YAAC_ROOT").filter(|v| !v.is_empty()).map_or_else(|| PathBuf::from("."), PathBuf::from)
}

/// This machine's host name, best effort.
pub fn host_name() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .unwrap_or_else(|| "host".to_owned())
}

impl HubConfig {
    /// A configuration from the process environment: [`HubPaths::from_env`], the local
    /// connector from `YAAC_RUNTIME` / `YAAC_DAEMON_BUNDLE` relative to [`default_root`].
    pub fn from_env(build: impl Into<String>) -> Result<Self, PathsError> {
        let paths = HubPaths::from_env()?;
        let local = LocalConfig::from_env(&default_root());
        let mut config = Self::new(build, paths, Arc::new(DefaultConnectors { local }));
        config.notifier = Some(Arc::new(SystemNotifier));
        Ok(config)
    }

    /// A configuration with default timings.
    pub fn new(build: impl Into<String>, paths: HubPaths, connectors: Arc<dyn ConnectorFactory>) -> Self {
        Self {
            build: build.into(),
            paths,
            connectors,
            idle_exit: None,
            backoff: Backoff::default(),
            ack_interval: Duration::from_millis(500),
            ping_interval: Duration::from_secs(15),
            dead_after: Duration::from_secs(45),
            cache_cap_bytes: 256 * 1024 * 1024,
            rebuild_cap_bytes: 8 * 1024 * 1024,
            seed_local_target: true,
            first_run_always: true,
            test_methods: false,
            host_name: host_name(),
            handle_signals: true,
            undelivered_notice: Duration::from_secs(15 * 60),
            notice_sweep: Duration::from_secs(60),
            notifier: None,
            uploads: crate::UploadLimits::default(),
            pool_threads: 2,
        }
    }
}
