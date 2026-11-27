//! How a hub is configured: paths, build, connectors, and timings.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use yaac_protocol::attach::TargetTransport;
use yaac_transport::wsl::{Distro, ListError, list_distros};
use yaac_transport::{BoxFuture, Connector, DaemonBundle, LocalConfig, LocalConnector, RuntimeSource, WslConfig, WslConnector};

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

/// Lists the WSL distros installed on this PC (`hub/wsl/distros`). Starts nothing. Tests
/// supply a fixed list.
pub trait WslLister: Send + Sync {
    /// The distros, in the order `wsl.exe` lists them.
    fn list(&self) -> BoxFuture<'static, Result<Vec<Distro>, ListError>>;
}

/// The default lister: `wsl.exe -l -v` on Windows (or the `wsl.exe` of
/// [`WslDefaults::wsl_exe`] anywhere); an empty list on a host that is not Windows.
#[derive(Clone, Debug, Default)]
pub struct DefaultWslLister {
    /// `wsl.exe` (default: found on `PATH`, on Windows only).
    pub wsl_exe: Option<std::ffi::OsString>,
}

impl WslLister for DefaultWslLister {
    fn list(&self) -> BoxFuture<'static, Result<Vec<Distro>, ListError>> {
        let exe = self.wsl_exe.clone().or_else(|| cfg!(windows).then(|| "wsl.exe".into()));
        Box::pin(async move {
            match exe {
                Some(exe) => list_distros(&exe).await,
                None => Ok(Vec::new()),
            }
        })
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
            TargetTransport::WindowsInterop(w) => {
                if !crate::transfer::running_in_wsl() {
                    return Err("This PC's Windows side is reached only from a hub in its WSL".into());
                }
                Ok(Arc::new(crate::interop::InteropConnector { exe: w.exe.clone() }))
            }
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

/// `wsl.exe` for a hub inside WSL (interop): Windows' own, else `wsl.exe` on `PATH`. `None`
/// outside WSL.
fn wsl_exe_in_wsl() -> Option<std::ffi::OsString> {
    if !crate::transfer::running_in_wsl() {
        return None;
    }
    let system = std::path::Path::new("/mnt/c/Windows/System32/wsl.exe");
    Some(if system.is_file() { system.into() } else { "wsl.exe".into() })
}

#[cfg(target_os = "linux")]
struct LinuxConnectors {
    defaults: DefaultConnectors,
    local: yaac_transport::local::LinuxLocalConnector,
}

#[cfg(target_os = "linux")]
impl ConnectorFactory for LinuxConnectors {
    fn connector(&self, target: &TargetDef) -> Result<Arc<dyn Connector>, String> {
        match &target.transport {
            TargetTransport::Local(_) => Ok(Arc::new(self.local.clone())),
            _ => self.defaults.connector(target),
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
    /// Add the default local target on first run. On Windows, also add it to existing settings
    /// from builds that lacked a native daemon.
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
    /// Desktop notifications for pending requests outside the focused window. `None`:
    /// off (the default of [`HubConfig::new`], and so of tests).
    pub notifier: Option<Arc<dyn Notifier>>,
    /// Upload limits (docs/06-attachments.md).
    pub uploads: crate::UploadLimits,
    /// Threads in the worker pool that reads and hashes files for uploads.
    pub pool_threads: usize,
    /// How SSH targets are reached and set up (`hub/ssh/*`). `None`: SSH targets stay offline
    /// with a message (the default of [`HubConfig::new`]; [`HubConfig::from_env`] sets it).
    pub ssh: Option<Arc<SshDefaults>>,
    /// Lists the installed WSL distros (`hub/wsl/distros`).
    pub wsl_distros: Arc<dyn WslLister>,
    /// What Tailscale says about this host and its peers, for attaching over Tailscale
    /// (the `tailscale` command line by default; tests supply a fake).
    pub tailnet: Arc<dyn crate::tailnet::Tailnet>,
    /// The port of the Tailscale attach listener ([`crate::tailnet::TAILSCALE_ATTACH_PORT`];
    /// tests use `0`, any free port).
    pub tailscale_port: u16,
    /// Installs another YAAC build on this computer when a newer phone asks
    /// (`hub/update/request`, [`crate::host_update`]). `None`: this host does not support it
    /// (the default; the `yaac` binary supplies one on Windows).
    pub host_installer: Option<Arc<dyn crate::host_update::HostInstaller>>,
    /// Finds this build's Linux hub in its release and turns start-at-login off, for moving the
    /// hub to another machine (`hub/transfer/*`, [`crate::transfer`]). `None`: this hub cannot
    /// move (the default; the `yaac` binary supplies one).
    pub transfer_host: Option<Arc<dyn crate::transfer::TransferHost>>,
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

/// How this computer's own `yaacd` is started: [`LocalConfig::from_env`], and on an installed
/// Windows build its packaged runtime and bundle, copied into versioned data directories
/// first. The Windows hub uses it, and so does `yaac.exe local-daemon` for a hub that moved
/// into this PC's WSL.
pub fn local_config(paths: &HubPaths) -> LocalConfig {
    let mut local = LocalConfig::from_env(&default_root());
    if cfg!(windows) && let Some(install) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(std::path::Path::to_path_buf)) {
        let home = std::env::var_os("YAAC_HOME").filter(|v| !v.is_empty()).map(PathBuf::from)
            .or_else(|| std::env::var_os("USERPROFILE").map(|profile| PathBuf::from(profile).join(".yaac")))
            .unwrap_or_else(|| paths.data_dir.clone());
        local = local.with_env("YAAC_HOME", &home);
        let runtime = install.join("runtime").join("node.exe");
        if std::env::var_os("YAAC_RUNTIME").is_none_or(|v| v.is_empty()) && runtime.is_file() {
            let dest = home.join("runtime").join(format!("node-{}", yaac_transport::runtime::PINNED_NODE)).join("node.exe");
            local.install_files.push((runtime, dest.clone()));
            let pty = "pty-1.2.0-beta.18";
            let packaged = install.join("runtime").join(pty);
            if packaged.is_dir() {
                local.install_files.push((packaged, dest.parent().unwrap().join(pty)));
            }
            local.runtime = dest.into_os_string();
        }
        let bundle = install.join("yaacd.mjs");
        if std::env::var_os("YAAC_DAEMON_BUNDLE").is_none_or(|v| v.is_empty()) && bundle.is_file() {
            let build = env!("YAAC_DAEMON_VERSION");
            let dest = home.join("daemon").join(build).join("yaacd.mjs");
            local.install_files.push((bundle, dest.clone()));
            local.bundle = dest;
            local.build = Some(build.to_owned());
        }
    }
    local
}

impl HubConfig {
    /// A configuration from the process environment: [`HubPaths::from_env`], the local
    /// connector from `YAAC_RUNTIME` / `YAAC_DAEMON_BUNDLE` relative to [`default_root`].
    pub fn from_env(build: impl Into<String>) -> Result<Self, PathsError> {
        let paths = HubPaths::from_env()?;
        let local = local_config(&paths);
        let mut wsl = WslDefaults::from_lookup(|k| std::env::var_os(k));
        // A hub that moved into WSL reaches the PC's other distros through `wsl.exe` (interop).
        if wsl.wsl_exe.is_none() {
            wsl.wsl_exe = wsl_exe_in_wsl();
        }
        let runtime = RuntimeSource::pinned(Some(paths.data_dir.join("runtime-cache")));
        wsl.runtime = Some(runtime.clone());
        let mut ssh = SshDefaults::new(&paths.data_dir, &paths.runtime_dir, wsl.bundle.clone(), wsl.env.clone());
        ssh.runtime = Some(runtime.clone());
        let lister = DefaultWslLister { wsl_exe: wsl.wsl_exe.clone() };
        let defaults = DefaultConnectors { local, wsl };
        let connectors: Arc<dyn ConnectorFactory> = {
            #[cfg(target_os = "linux")]
            if ["YAAC_RUNTIME", "YAAC_DAEMON_BUNDLE", "YAAC_ROOT"].iter().all(|key| std::env::var_os(key).is_none_or(|v| v.is_empty()))
                && let Some(bundle) = defaults.wsl.bundle.clone()
            {
                Arc::new(LinuxConnectors {
                    local: yaac_transport::local::LinuxLocalConnector { bundle, runtime, env: defaults.wsl.env.clone() },
                    defaults,
                })
            } else {
                Arc::new(defaults)
            }
            #[cfg(not(target_os = "linux"))]
            Arc::new(defaults)
        };
        let mut config = Self::new(build, paths, connectors);
        config.wsl_distros = Arc::new(lister);
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
            ssh: None,
            wsl_distros: Arc::new(DefaultWslLister::default()),
            tailnet: Arc::new(crate::tailnet::TailscaleCli::default()),
            tailscale_port: crate::tailnet::TAILSCALE_ATTACH_PORT,
            host_installer: None,
            transfer_host: None,
        }
    }
}
