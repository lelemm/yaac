//! Starting the hub at login (the hub runs always by default — docs/10-agent-mesh.md, "The
//! hub").
//!
//! The library never calls these on its own: the application installs the entry on first
//! run and from its setting. Linux uses an XDG autostart entry,
//! `$XDG_CONFIG_HOME/autostart/yaac-hub.desktop` (else `~/.config/autostart`); the directory
//! is overridable with `YAAC_AUTOSTART_DIR`, or explicitly through [`Autostart::new`], so
//! tests never touch the real one. Other platforms report [`AutostartError::Unsupported`]
//! until their implementation (a Run registry value or Startup shortcut on Windows) lands
//! behind the same functions.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The file name of the entry.
pub const ENTRY_NAME: &str = "yaac-hub.desktop";

/// Where the entry goes and what it runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Autostart {
    /// The autostart directory.
    pub dir: PathBuf,
    /// The `yaac` binary.
    pub exe: PathBuf,
    /// Arguments (default: `hub`).
    pub args: Vec<String>,
}

/// An autostart operation failed.
#[derive(Debug, thiserror::Error)]
pub enum AutostartError {
    /// Not implemented on this platform yet.
    #[error("starting the hub at login is not supported on this platform yet")]
    Unsupported,
    /// The autostart directory could not be determined.
    #[error("cannot determine the autostart directory: set YAAC_AUTOSTART_DIR")]
    NoDir,
    /// I/O.
    #[error("autostart entry {path}: {source}")]
    Io {
        /// The file.
        path: String,
        /// Cause.
        #[source]
        source: std::io::Error,
    },
}

impl Autostart {
    /// An entry in `dir` running `exe hub`.
    pub fn new(dir: impl Into<PathBuf>, exe: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into(), exe: exe.into(), args: vec!["hub".into()] }
    }

    /// From the environment: `YAAC_AUTOSTART_DIR`, else `$XDG_CONFIG_HOME/autostart`, else
    /// `~/.config/autostart`.
    pub fn from_env(exe: impl Into<PathBuf>) -> Result<Self, AutostartError> {
        Self::from_lookup(exe, |k| std::env::var_os(k))
    }

    /// From any variable lookup.
    pub fn from_lookup(exe: impl Into<PathBuf>, lookup: impl Fn(&str) -> Option<OsString>) -> Result<Self, AutostartError> {
        let get = |k: &str| lookup(k).filter(|v| !v.is_empty()).map(PathBuf::from);
        let dir = get("YAAC_AUTOSTART_DIR")
            .or_else(|| get("XDG_CONFIG_HOME").map(|d| d.join("autostart")))
            .or_else(|| get("HOME").map(|h| h.join(".config").join("autostart")))
            .ok_or(AutostartError::NoDir)?;
        Ok(Self::new(dir, exe))
    }

    /// The entry's path.
    pub fn entry(&self) -> PathBuf {
        self.dir.join(ENTRY_NAME)
    }

    fn desktop_file(&self) -> String {
        let quote = |s: &str| {
            if s.chars().any(|c| c.is_whitespace() || "\"'\\$`".contains(c)) {
                format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"").replace('$', "\\$").replace('`', "\\`"))
            } else {
                s.to_owned()
            }
        };
        let mut exec = quote(&self.exe.to_string_lossy());
        for arg in &self.args {
            exec.push(' ');
            exec.push_str(&quote(arg));
        }
        format!(
            "[Desktop Entry]\nType=Application\nName=YAAC hub\nComment=Keeps agents on your targets connected\nExec={exec}\nTerminal=false\nNoDisplay=true\nX-GNOME-Autostart-enabled=true\n"
        )
    }
}

/// Installs (or rewrites) the login entry.
pub fn install_autostart(autostart: &Autostart) -> Result<(), AutostartError> {
    if !cfg!(all(unix, not(target_os = "macos"))) {
        return Err(AutostartError::Unsupported);
    }
    let path = autostart.entry();
    let io = |source| AutostartError::Io { path: path.display().to_string(), source };
    std::fs::create_dir_all(&autostart.dir).map_err(io)?;
    let tmp = path.with_extension("desktop.tmp");
    std::fs::write(&tmp, autostart.desktop_file()).map_err(io)?;
    std::fs::rename(&tmp, &path).map_err(io)
}

/// Removes the login entry (nothing to do if it is absent).
pub fn remove_autostart(autostart: &Autostart) -> Result<(), AutostartError> {
    if !cfg!(all(unix, not(target_os = "macos"))) {
        return Err(AutostartError::Unsupported);
    }
    let path = autostart.entry();
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(AutostartError::Io { path: path.display().to_string(), source }),
    }
}

/// Whether the login entry is installed.
pub fn autostart_installed(autostart: &Autostart) -> bool {
    Path::new(&autostart.entry()).is_file()
}

#[cfg(all(test, unix, not(target_os = "macos")))]
mod tests {
    use super::*;

    #[test]
    fn install_query_remove_in_a_private_directory() {
        let dir = std::env::temp_dir().join(format!("yaac-autostart-{}", std::process::id()));
        let a = Autostart::new(dir.join("autostart"), "/opt/My Apps/yaac");
        assert!(!autostart_installed(&a));
        install_autostart(&a).unwrap();
        assert!(autostart_installed(&a));
        let text = std::fs::read_to_string(a.entry()).unwrap();
        assert!(text.contains("Exec=\"/opt/My Apps/yaac\" hub\n"), "{text}");
        install_autostart(&a).unwrap(); // idempotent
        remove_autostart(&a).unwrap();
        assert!(!autostart_installed(&a));
        remove_autostart(&a).unwrap(); // absent is fine
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_directory_follows_the_environment() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| pairs.iter().find(|(key, _)| *key == k).map(|(_, v)| OsString::from(v))
        };
        let a = Autostart::from_lookup("/y", env(&[("HOME", "/h")])).unwrap();
        assert_eq!(a.entry(), PathBuf::from("/h/.config/autostart/yaac-hub.desktop"));
        let a = Autostart::from_lookup("/y", env(&[("HOME", "/h"), ("XDG_CONFIG_HOME", "/x")])).unwrap();
        assert_eq!(a.dir, PathBuf::from("/x/autostart"));
        let a = Autostart::from_lookup("/y", env(&[("HOME", "/h"), ("YAAC_AUTOSTART_DIR", "/t")])).unwrap();
        assert_eq!(a.dir, PathBuf::from("/t"));
    }
}
