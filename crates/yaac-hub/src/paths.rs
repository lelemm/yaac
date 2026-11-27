//! Where the hub keeps its files, and how the environment moves them.
//!
//! | What | Default (Linux) | Override |
//! | --- | --- | --- |
//! | Config directory — `hub.json` (targets, hub settings) | `$XDG_CONFIG_HOME/yaac`, else `~/.config/yaac` | `YAAC_CONFIG_DIR` |
//! | Data directory — `cache.sqlite`, `drafts.sqlite`, `relay.sqlite` | `$XDG_DATA_HOME/yaac`, else `~/.local/share/yaac` | `YAAC_DATA_DIR` |
//! | Runtime directory — `hub.sock`, `hub.lock` | `$XDG_RUNTIME_DIR/yaac`, else `<data>/run` | `YAAC_RUNTIME_DIR` |
//!
//! When `YAAC_DATA_DIR` is set and `YAAC_RUNTIME_DIR` is not, the runtime directory is
//! `<data>/run`: an isolated data directory (a test, a second profile) never shares a socket
//! or a lock with the user's real hub. macOS uses `~/Library/Application Support/yaac` for
//! both config and data; Windows uses `%APPDATA%\yaac` and `%LOCALAPPDATA%\yaac`.
//!
//! The window resolves the same paths with [`HubPaths::from_env`] to find the socket and the
//! cache file.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The longest socket path accepted (`sun_path` is 108 bytes on Linux, 104 on macOS).
pub const MAX_SOCKET_PATH: usize = 100;

/// The hub's directories.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HubPaths {
    /// Settings (`hub.json`).
    pub config_dir: PathBuf,
    /// SQLite stores.
    pub data_dir: PathBuf,
    /// Socket and lock.
    pub runtime_dir: PathBuf,
}

/// The paths could not be determined.
#[derive(Debug, thiserror::Error)]
#[error("cannot determine the {0} directory: set {1}")]
pub struct PathsError(pub &'static str, pub &'static str);

impl HubPaths {
    /// All three under one root (tests, portable installs): `<root>/config`, `<root>/data`,
    /// `<root>/run`.
    pub fn under(root: &Path) -> Self {
        Self { config_dir: root.join("config"), data_dir: root.join("data"), runtime_dir: root.join("run") }
    }

    /// From the process environment.
    pub fn from_env() -> Result<Self, PathsError> {
        Self::from_lookup(|key| std::env::var_os(key))
    }

    /// From any variable lookup (testable without touching the process environment).
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<OsString>) -> Result<Self, PathsError> {
        let get = |key: &str| lookup(key).filter(|v| !v.is_empty()).map(PathBuf::from);
        let home = get("HOME").or_else(|| get("USERPROFILE"));
        let config_dir = match get("YAAC_CONFIG_DIR") {
            Some(dir) => dir,
            None => platform_config(&get, home.as_deref()).ok_or(PathsError("config", "YAAC_CONFIG_DIR"))?,
        };
        let explicit_data = get("YAAC_DATA_DIR");
        let data_dir = match &explicit_data {
            Some(dir) => dir.clone(),
            None => platform_data(&get, home.as_deref()).ok_or(PathsError("data", "YAAC_DATA_DIR"))?,
        };
        let runtime_dir = match get("YAAC_RUNTIME_DIR") {
            Some(dir) => dir,
            None if explicit_data.is_some() => data_dir.join("run"),
            None => get("XDG_RUNTIME_DIR").map_or_else(|| data_dir.join("run"), |dir| dir.join("yaac")),
        };
        Ok(Self { config_dir, data_dir, runtime_dir })
    }

    /// The settings file.
    pub fn settings_file(&self) -> PathBuf {
        self.config_dir.join("hub.json")
    }

    /// The disposable cache.
    pub fn cache_file(&self) -> PathBuf {
        self.data_dir.join("cache.sqlite")
    }

    /// The durable draft store.
    pub fn drafts_file(&self) -> PathBuf {
        self.data_dir.join("drafts.sqlite")
    }

    /// The durable mesh relay queue.
    pub fn relay_file(&self) -> PathBuf {
        self.data_dir.join("relay.sqlite")
    }

    /// Records the new address after this hub has moved.
    pub fn moved_file(&self) -> PathBuf {
        self.data_dir.join("moved.json")
    }

    /// The attach socket: `<runtime>/hub.sock` on Unix. On Windows, a named pipe,
    /// `\\.\pipe\yaac-hub-<user SID>-<runtime hash>`: pipes live in one machine-wide namespace,
    /// so the name carries the user, and a hash of the runtime directory keeps a second
    /// profile (tests, `YAAC_RUNTIME_DIR`) off the user's real hub, exactly as a separate
    /// socket file does on Unix.
    #[cfg(not(windows))]
    pub fn socket(&self) -> PathBuf {
        self.runtime_dir.join("hub.sock")
    }

    /// See the Unix variant.
    #[cfg(windows)]
    pub fn socket(&self) -> PathBuf {
        let user = yaac_winsys::current_user_sid()
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "user".to_owned());
        PathBuf::from(format!(r"\\.\pipe\yaac-hub-{user}-{}", pipe_suffix(&self.runtime_dir)))
    }

    /// The single-hub lock.
    pub fn lock_file(&self) -> PathBuf {
        self.runtime_dir.join("hub.lock")
    }
}

/// The destination of a hub that stopped on this machine after transferring.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MovedTombstone {
    pub hub_id: String,
    pub address: String,
    pub host_name: String,
    pub at_ms: i64,
}

/// Reads the move record. An unreadable or malformed record is an error.
pub fn moved_tombstone(paths: &HubPaths) -> std::io::Result<Option<MovedTombstone>> {
    match std::fs::read(paths.moved_file()) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(std::io::Error::other),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Publishes the move record atomically. The caller holds the hub lock.
pub fn write_moved_tombstone(paths: &HubPaths, moved: &MovedTombstone) -> std::io::Result<()> {
    use std::io::Write;

    let path = paths.moved_file();
    let tmp = path.with_extension("json.tmp");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp)?;
    make_private_file(&tmp)?;
    file.write_all(&serde_json::to_vec(moved).map_err(std::io::Error::other)?)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(tmp, path)
}

/// Removes the move record when reclaiming this hub.
pub fn remove_moved_tombstone(paths: &HubPaths) -> std::io::Result<bool> {
    match std::fs::remove_file(paths.moved_file()) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// A short, stable hash of a runtime directory (FNV-1a over its lower-cased text with one kind
/// of separator, since Windows paths are case-insensitive and accept both) for the pipe name.
#[cfg(any(windows, test))]
fn pipe_suffix(runtime_dir: &Path) -> String {
    let text = runtime_dir.to_string_lossy().to_lowercase().replace('/', "\\");
    let text = text.trim_end_matches('\\');
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{:012x}", hash >> 16)
}

fn platform_config(get: &impl Fn(&str) -> Option<PathBuf>, home: Option<&Path>) -> Option<PathBuf> {
    if cfg!(windows) {
        return get("APPDATA").map(|d| d.join("yaac"));
    }
    if cfg!(target_os = "macos") {
        return home.map(|h| h.join("Library").join("Application Support").join("yaac"));
    }
    get("XDG_CONFIG_HOME").or_else(|| home.map(|h| h.join(".config"))).map(|d| d.join("yaac"))
}

fn platform_data(get: &impl Fn(&str) -> Option<PathBuf>, home: Option<&Path>) -> Option<PathBuf> {
    if cfg!(windows) {
        return get("LOCALAPPDATA").map(|d| d.join("yaac"));
    }
    if cfg!(target_os = "macos") {
        return home.map(|h| h.join("Library").join("Application Support").join("yaac"));
    }
    get("XDG_DATA_HOME").or_else(|| home.map(|h| h.join(".local").join("share"))).map(|d| d.join("yaac"))
}

/// Creates a directory (and parents) readable only by the user.
pub(crate) fn ensure_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Restricts a file to the user.
pub(crate) fn make_private_file(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let pairs: Vec<(String, String)> = pairs.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect();
        move |key| pairs.iter().find(|(k, _)| k == key).map(|(_, v)| OsString::from(v))
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_defaults_follow_xdg() {
        let p = HubPaths::from_lookup(lookup(&[("HOME", "/h/u"), ("XDG_RUNTIME_DIR", "/run/user/1")])).unwrap();
        assert_eq!(p.config_dir, PathBuf::from("/h/u/.config/yaac"));
        assert_eq!(p.data_dir, PathBuf::from("/h/u/.local/share/yaac"));
        assert_eq!(p.socket(), PathBuf::from("/run/user/1/yaac/hub.sock"));
        let p = HubPaths::from_lookup(lookup(&[("HOME", "/h/u")])).unwrap();
        assert_eq!(p.runtime_dir, PathBuf::from("/h/u/.local/share/yaac/run"));
    }

    #[test]
    fn the_pipe_suffix_is_stable_and_separates_profiles() {
        let a = pipe_suffix(Path::new(r"C:\Users\u\AppData\Local\yaac\run"));
        assert_eq!(a, pipe_suffix(Path::new(r"c:\users\U\AppData\Local\yaac\run\")), "case and a trailing separator do not matter");
        assert_eq!(a, pipe_suffix(Path::new("C:/Users/u/AppData/Local/yaac/run")), "nor the kind of separator");
        assert_ne!(a, pipe_suffix(Path::new(r"C:\Temp\t1\data\run")));
        assert_eq!(a.len(), 12);
    }

    #[test]
    fn overrides_win_and_an_explicit_data_dir_isolates_the_socket() {
        let p = HubPaths::from_lookup(lookup(&[
            ("HOME", "/h/u"),
            ("XDG_RUNTIME_DIR", "/run/user/1"),
            ("YAAC_CONFIG_DIR", "/t/c"),
            ("YAAC_DATA_DIR", "/t/d"),
        ]))
        .unwrap();
        assert_eq!(p.settings_file(), PathBuf::from("/t/c/hub.json"));
        assert_eq!(p.cache_file(), PathBuf::from("/t/d/cache.sqlite"));
        #[cfg(not(windows))]
        assert_eq!(p.socket(), PathBuf::from("/t/d/run/hub.sock"), "never the real runtime dir");
        #[cfg(windows)]
        assert!(p.socket().to_string_lossy().ends_with(&pipe_suffix(Path::new("/t/d/run"))), "never the real hub's pipe");
        let p = HubPaths::from_lookup(lookup(&[("YAAC_CONFIG_DIR", "/c"), ("YAAC_DATA_DIR", "/d"), ("YAAC_RUNTIME_DIR", "/r")]))
            .unwrap();
        assert_eq!(p.lock_file(), PathBuf::from("/r/hub.lock"));
    }
}
