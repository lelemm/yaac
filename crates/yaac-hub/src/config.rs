//! How a hub is configured: paths, build, connectors, and timings.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use yaac_protocol::attach::TargetTransport;
use yaac_transport::{Connector, DaemonBundle, LocalConfig, LocalConnector, RuntimeSource, WslConfig, WslConnector};

use crate::notify::{Notifier, SystemNotifier};
use crate::paths::{HubPaths, PathsError};
use crate::settings::TargetDef;
use crate::ssh::SshDefaults;

/// Makes the connector for a target. The default builds a [`LocalConnector`] for local
/// targets and a [`WslConnector`] for WSL targets; SSH targets are built by the hub itself
/// from [`HubConfig::ssh`] (their key and setup state are the hub's); tests supply in-memory
/// daemons.
pub trait ConnectorFactory: Send + Sync {
    /// The connector for this target, or a message to show as the target's state.
    fn connector(&self, target: &TargetDef) -> Result<Arc<dyn Connector>, String>;
}

/// The `yaacd` bundle built into this hub (`daemon/dist/yaacd.mjs` when the hub was built),
/// which stage 0 installs on remote targets that lack its build. `None` when the hub was built
/// without one.
pub fn embedded_bundle() -> Option<DaemonBundle> {
    #[cfg(yaac_daemon_bundle)]
    {
        static BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/yaacd.mjs"));
        Some(DaemonBundle::new(env!("YAAC_DAEMON_VERSION"), BYTES.to_vec()))
    }
    #[cfg(not(yaac_daemon_bundle))]
    None
}

/// How WSL targets are reached.
#[derive(Clone, Debug, Default)]
pub struct WslDefaults {
    /// `wsl.exe` (default: found on `PATH`).
    pub wsl_exe: Option<std::ffi::OsString>,
    /// The bundle to install.
    pub bundle: Option<DaemonBundle>,
    /// Variables stage 0 exports on the target before the daemon starts (`YAAC_` names only).
    pub env: Vec<(String, String)>,
    /// Where a distro with no runtime gets one (`None`: it needs a Node.js 22+ of its own).
    pub runtime: Option<RuntimeSource>,
}

impl WslDefaults {
    /// From the environment: the bundle is `YAAC_DAEMON_BUNDLE` when set (development), else
    /// [`embedded_bundle`]; `YAAC_TARGET_ENV` (`NAME=value;NAME=value`, `YAAC_` names only)
    /// passes variables to the daemon on remote targets — a development and test hook, for
    /// example a temporary `YAAC_HOME` and the fake provider.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<std::ffi::OsString>) -> Self {
        let get = |k: &str| lookup(k).filter(|v| !v.is_empty());
        let bundle = get("YAAC_DAEMON_BUNDLE")
            .and_then(|path| std::fs::read(path).ok())
            .map(|bytes| DaemonBundle::new(env!("YAAC_DAEMON_VERSION"), bytes))
            .or_else(embedded_bundle);
        let env = get("YAAC_TARGET_ENV")
            .map(|v| {
                v.to_string_lossy()
                    .split(';')
                    .filter_map(|kv| kv.split_once('='))
                    .filter(|(k, _)| k.starts_with("YAAC_") && k.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_'))
                    .map(|(k, v)| (k.to_owned(), v.to_owned()))
                    .collect()
            })
            .unwrap_or_default();
        Self { wsl_exe: get("YAAC_WSL_EXE"), bundle, env, runtime: None }
    }
}

/// The default connector factory.
#[derive(Clone, Debug)]
pub struct DefaultConnectors {
    /// How the local runtime and bundle are found.
    pub local: LocalConfig,
    /// How WSL targets are reached.
    pub wsl: WslDefaults,
}

impl ConnectorFactory for DefaultConnectors {
    fn connector(&self, target: &TargetDef) -> Result<Arc<dyn Connector>, String> {
        match &target.transport {
            TargetTransport::Local(_) => Ok(Arc::new(LocalConnector::new(self.local.clone()))),
            // The hub builds SSH connectors itself, from `HubConfig::ssh`.
            TargetTransport::Ssh(_) => Err("SSH is not configured in this hub".into()),
            TargetTransport::Wsl(w) => {
                if !cfg!(windows) && self.wsl.wsl_exe.is_none() {
                    return Err("WSL targets are reached from a Windows host".into());
                }
                let mut config = WslConfig::new(&w.distro, w.user.clone(), self.wsl.bundle.clone());
                if let Some(exe) = &self.wsl.wsl_exe {
                    config.wsl_exe.clone_from(exe);
                }
                config.env.clone_from(&self.wsl.env);
                config.runtime.clone_from(&self.wsl.runtime);
                Ok(Arc::new(WslConnector::new(config)))
            }
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
    /// On first run, add an enabled local target named `local`. Off by default on Windows:
    /// the daemon's runtime and harnesses are not assumed to exist natively there; agents run
    /// in WSL distros, which the user adds.
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
    /// How SSH targets are reached and set up (`hub/ssh/*`). `None`: SSH targets stay offline
    /// with a message (the default of [`HubConfig::new`]; [`HubConfig::from_env`] sets it).
    pub ssh: Option<Arc<SshDefaults>>,
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
        let mut wsl = WslDefaults::from_lookup(|k| std::env::var_os(k));
        let runtime = RuntimeSource::pinned(Some(paths.data_dir.join("runtime-cache")));
        wsl.runtime = Some(runtime.clone());
        let mut ssh = SshDefaults::new(&paths.data_dir, &paths.runtime_dir, wsl.bundle.clone(), wsl.env.clone());
        ssh.runtime = Some(runtime);
        let mut config = Self::new(build, paths, Arc::new(DefaultConnectors { local, wsl }));
        config.ssh = Some(Arc::new(ssh));
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
            seed_local_target: !cfg!(windows),
            first_run_always: true,
            test_methods: false,
            host_name: host_name(),
            handle_signals: true,
            undelivered_notice: Duration::from_secs(15 * 60),
            notice_sweep: Duration::from_secs(60),
            notifier: None,
            uploads: crate::UploadLimits::default(),
            pool_threads: 2,
            ssh: None,
        }
    }
}
