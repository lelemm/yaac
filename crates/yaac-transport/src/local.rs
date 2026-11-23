//! The local connector: `<private runtime> <bundle> connect`, with no shell in between
//! (docs/02-architecture.md#the-single-connection-rule).
//!
//! An installed Windows build copies its private runtime and bundle into versioned data
//! directories. Development builds find them through configuration:
//!
//! | Variable | Default |
//! | --- | --- |
//! | `YAAC_RUNTIME` | `node` (looked up on `PATH`) |
//! | `YAAC_DAEMON_BUNDLE` | `<root>/daemon/dist/yaacd.mjs` |
//!
//! `<root>` is supplied by the caller (the repository root during development).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::process::Command;

use crate::{BoxFuture, ChildGuard, ConnectCtx, ConnectError, Connector, Stream};

fn install_file(source: &Path, dest: &Path) -> std::io::Result<()> {
    if dest.is_file() {
        return Ok(());
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let part = dest.with_extension(format!("part-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    let result = std::fs::copy(source, &part).and_then(|_| std::fs::rename(&part, dest));
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    if dest.is_file() { Ok(()) } else { result }
}

/// Where the runtime and bundle are, and how the child is started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalConfig {
    /// The JavaScript runtime binary (a path, or a name looked up on `PATH`).
    pub runtime: OsString,
    /// The `yaacd` bundle.
    pub bundle: PathBuf,
    /// Extra environment for the child (added to the inherited environment).
    pub env: Vec<(OsString, OsString)>,
    /// Whether the child's standard error is passed through (otherwise discarded).
    pub inherit_stderr: bool,
    /// The bundle's build (`<version>+<id>`), when the configuration knows it
    /// ([`Connector::daemon_build`]). `None`: unknown, and the hub never asks an older local
    /// daemon to hand over.
    pub build: Option<String>,
    /// Files copied from the installer into the data directory before the first connection.
    pub install_files: Vec<(PathBuf, PathBuf)>,
}

impl LocalConfig {
    /// The bundle's default location relative to a root directory.
    pub fn default_bundle(root: &Path) -> PathBuf {
        root.join("daemon").join("dist").join("yaacd.mjs")
    }

    /// From the process environment (`YAAC_RUNTIME`, `YAAC_DAEMON_BUNDLE`).
    pub fn from_env(root: &Path) -> Self {
        Self::from_lookup(root, |key| std::env::var_os(key))
    }

    /// From any variable lookup (testable without touching the process environment).
    pub fn from_lookup(root: &Path, lookup: impl Fn(&str) -> Option<OsString>) -> Self {
        let runtime = lookup("YAAC_RUNTIME").filter(|v| !v.is_empty()).unwrap_or_else(|| OsString::from("node"));
        let bundle = lookup("YAAC_DAEMON_BUNDLE")
            .filter(|v| !v.is_empty())
            .map_or_else(|| Self::default_bundle(root), PathBuf::from);
        Self { runtime, bundle, env: Vec::new(), inherit_stderr: true, build: None, install_files: Vec::new() }
    }

    /// Declares the bundle's build ([`LocalConfig::build`]).
    #[must_use]
    pub fn with_build(mut self, build: impl Into<String>) -> Self {
        self.build = Some(build.into());
        self
    }

    /// Adds an environment variable for the child.
    #[must_use]
    pub fn with_env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }
}

/// Spawns the local `yaacd connect` bridge. See the module docs.
#[derive(Clone, Debug)]
pub struct LocalConnector {
    config: LocalConfig,
}

impl LocalConnector {
    /// A connector for this configuration.
    pub fn new(config: LocalConfig) -> Self {
        Self { config }
    }

    /// The configuration.
    pub fn config(&self) -> &LocalConfig {
        &self.config
    }

    fn spawn(&self) -> Result<Stream, ConnectError> {
        let config = &self.config;
        if !config.bundle.is_file() {
            return Err(ConnectError::NotInstalled(format!(
                "the yaacd bundle was not found at {} (set YAAC_DAEMON_BUNDLE)",
                config.bundle.display()
            )));
        }
        let mut command = Command::new(&config.runtime);
        command
            .arg(&config.bundle)
            .arg("connect")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(if config.inherit_stderr { Stdio::inherit() } else { Stdio::null() })
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(crate::CREATE_NO_WINDOW);
        for (key, value) in &config.env {
            command.env(key, value);
        }
        let mut child = command.spawn().map_err(|source| ConnectError::Spawn { what: self.describe(), source })?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            return Err(ConnectError::Other("the child's standard streams were not captured".into()));
        };
        Ok(Stream { reader: Box::new(stdout), writer: Box::new(stdin), child: ChildGuard::process(child) })
    }
}

impl Connector for LocalConnector {
    fn connect<'a>(&'a self, _ctx: &'a ConnectCtx) -> BoxFuture<'a, Result<Stream, ConnectError>> {
        Box::pin(async move {
            if !self.config.install_files.is_empty() {
                let files = self.config.install_files.clone();
                tokio::task::spawn_blocking(move || -> Result<(), ConnectError> {
                    for (source, dest) in files {
                        install_file(&source, &dest).map_err(|e| ConnectError::NotInstalled(format!("could not install {}: {e}", dest.display())))?;
                    }
                    Ok(())
                })
                .await
                .map_err(|e| ConnectError::Other(e.to_string()))??;
            }
            self.spawn()
        })
    }

    fn describe(&self) -> String {
        format!("local: {} {} connect", self.config.runtime.to_string_lossy(), self.config.bundle.display())
    }

    fn daemon_build(&self) -> Option<String> {
        self.config.build.clone()
    }
}
