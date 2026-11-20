//! One hub per user on the host: an exclusive lock on `<runtime>/hub.lock`, held for the
//! hub's lifetime and released by the operating system if the process dies.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use fs4::fs_std::FileExt;

/// The held lock. Dropping it releases the lock.
#[derive(Debug)]
pub struct HubLock {
    file: File,
    path: PathBuf,
}

/// Taking the lock failed.
#[derive(Debug, thiserror::Error)]
pub enum LockError {
    /// Another hub holds it.
    #[error("another hub is already running for this user (lock {0})")]
    Held(String),
    /// I/O.
    #[error("hub lock {path}: {source}")]
    Io {
        /// The file.
        path: String,
        /// Cause.
        #[source]
        source: std::io::Error,
    },
}

impl HubLock {
    /// Takes the lock, or fails at once if another hub holds it.
    pub fn acquire(path: &Path) -> Result<Self, LockError> {
        let io = |source| LockError::Io { path: path.display().to_string(), source };
        let mut file = OpenOptions::new().create(true).truncate(false).read(true).write(true).open(path).map_err(io)?;
        let _ = crate::paths::make_private_file(path);
        match FileExt::try_lock_exclusive(&file) {
            Ok(()) => {}
            Err(e) if e.kind() == fs4::lock_contended_error().kind() => {
                return Err(LockError::Held(path.display().to_string()));
            }
            Err(e) => return Err(io(e)),
        }
        // Diagnostics only: who holds it.
        let _ = file.set_len(0);
        let _ = writeln!(file, "{}", std::process::id());
        Ok(Self { file, path: path.to_owned() })
    }

    /// The lock file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for HubLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}
