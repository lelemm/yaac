//! The hub's settings file (`<config>/hub.json`): target definitions and hub settings.
//!
//! The hub is the file's only writer; the window changes it through the attach protocol.
//! Writes are atomic (temporary file, then rename) and user-only. Unknown fields are kept, so
//! a newer build's additions survive an older build rewriting the file.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use yaac_protocol::attach::{LocalTransport, TargetTransport};
use yaac_protocol::{ClientId, TargetId, TargetName};

use crate::paths::make_private_file;

/// A target as the settings file stores it. `id` is assigned by the hub and never changes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetDef {
    /// Immutable id, assigned by the hub.
    pub id: TargetId,
    /// The user's label; unique.
    pub name: TargetName,
    /// How it is reached.
    pub transport: TargetTransport,
    /// Whether the hub keeps it connected.
    pub enabled: bool,
    /// Fields this build does not know.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Hub behaviour settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HubSettings {
    /// Keep running even when idle with no window attached, started at login. **The
    /// default** (owner decision); `false` selects the idle-exit mode.
    #[serde(default = "default_always")]
    pub always: bool,
    /// How long the hub waits, idle and with no window attached, before exiting.
    #[serde(default = "default_idle_exit_secs")]
    pub idle_exit_secs: u64,
    #[serde(default)]
    pub explanations: yaac_protocol::ExplanationModels,
    #[serde(default)]
    pub notifications: yaac_protocol::attach::NotificationSettings,
    /// Accept attaches over Tailscale from this host's own Tailscale login ("Allow the Android
    /// client over Tailscale"). Off by default.
    #[serde(default)]
    pub tailscale_attach: bool,
    /// Fields this build does not know.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The default idle period before an unattended, idle hub exits.
pub const DEFAULT_IDLE_EXIT_SECS: u64 = 300;

fn default_always() -> bool {
    true
}

fn default_idle_exit_secs() -> u64 {
    DEFAULT_IDLE_EXIT_SECS
}

impl Default for HubSettings {
    fn default() -> Self {
        Self { always: true, idle_exit_secs: DEFAULT_IDLE_EXIT_SECS, explanations: Default::default(), notifications: Default::default(), tailscale_attach: false, extra: Map::new() }
    }
}

/// The whole file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// Targets, in the user's order.
    #[serde(default)]
    pub targets: Vec<TargetDef>,
    /// Hub settings.
    #[serde(default)]
    pub hub: HubSettings,
    /// This host's stable client id, sent to every `yaacd` in `initialize`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<ClientId>,
    /// This hub install's stable id, made on its first start and reported in `hub/hello`
    /// (`hubId`) so a client can tell two addresses of one hub apart from two hubs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub_id: Option<String>,
    /// What YAAC did on each SSH server during setup (docs/02-architecture.md#setting-up-an-ssh-connection),
    /// by *user@host:port*. Never a password; YAAC's own keys are files beside this one's data.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ssh: Vec<crate::protocol::SshRecordEntry>,
    /// Fields this build does not know.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Reading or writing the settings file failed.
#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    /// I/O.
    #[error("settings file {path}: {source}")]
    Io {
        /// The file.
        path: String,
        /// Cause.
        #[source]
        source: std::io::Error,
    },
    /// Not valid settings JSON. The file is left untouched.
    #[error("settings file {path} is not valid: {source}")]
    Invalid {
        /// The file.
        path: String,
        /// Cause.
        #[source]
        source: serde_json::Error,
    },
}

impl Settings {
    /// Loads the file. `Ok(None)` when it does not exist yet.
    pub fn load(path: &Path) -> Result<Option<Self>, SettingsError> {
        let text = match std::fs::read(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(SettingsError::Io { path: path.display().to_string(), source }),
        };
        serde_json::from_slice(&text)
            .map(Some)
            .map_err(|source| SettingsError::Invalid { path: path.display().to_string(), source })
    }

    /// Writes the file atomically, user-only.
    pub fn save(&self, path: &Path) -> Result<(), SettingsError> {
        let io = |source| SettingsError::Io { path: path.display().to_string(), source };
        let mut text = serde_json::to_vec_pretty(self).map_err(|e| io(std::io::Error::other(e)))?;
        text.push(b'\n');
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, &text).map_err(io)?;
        make_private_file(&tmp).map_err(io)?;
        std::fs::rename(&tmp, path).map_err(io)
    }

    /// A first-run file: one enabled local target named `local`, when `seed_local` is set;
    /// `always` seeds the lifecycle setting.
    pub fn first_run(seed_local: bool, always: bool) -> Self {
        let hub = HubSettings { always, ..HubSettings::default() };
        let mut settings = Self { targets: Vec::new(), hub, client_id: None, hub_id: None, ssh: Vec::new(), extra: Map::new() };
        if seed_local {
            settings.ensure_local_target();
        }
        settings
    }

    /// Add the built-in target to settings from Windows releases that did not register it.
    pub(crate) fn ensure_local_target(&mut self) {
        if self.targets.iter().any(|t| matches!(t.transport, TargetTransport::Local(_))) {
            return;
        }
        let name = std::iter::once("local".to_owned())
            .chain((2..).map(|n| format!("local ({n})")))
            .find(|name| self.name_is_free(name, None))
            .expect("a unique local target name");
        self.targets.insert(0, TargetDef {
            id: TargetId::new(new_id("t")),
            name: TargetName::new(name),
            transport: TargetTransport::Local(LocalTransport {}),
            enabled: true,
            extra: Map::new(),
        });
    }

    /// The target with this id.
    pub fn target(&self, id: &TargetId) -> Option<&TargetDef> {
        self.targets.iter().find(|t| &t.id == id)
    }

    /// Whether `name` is free (ignoring `except`). Names compare trimmed and case-insensitively.
    pub fn name_is_free(&self, name: &str, except: Option<&TargetId>) -> bool {
        let wanted = name.trim().to_lowercase();
        !self.targets.iter().any(|t| Some(&t.id) != except && t.name.as_str().trim().to_lowercase() == wanted)
    }
}

/// A new opaque id: `<prefix>` followed by 24 hex digits. Contains no `/`, so it is safe in
/// mesh addresses (`<target>/<thread>`).
pub fn new_id(prefix: &str) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    // `RandomState` is seeded randomly per process; hashing time, pid, and a counter with it
    // gives ids that do not collide across processes or restarts.
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u128(nanos);
    hasher.write_u32(std::process::id());
    hasher.write_u64(count);
    let high = hasher.finish();
    hasher.write_u8(0x5a);
    let low = hasher.finish() & 0xffff_ffff;
    format!("{prefix}{high:016x}{low:08x}")
}
