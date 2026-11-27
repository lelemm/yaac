//! Updating YAAC on the hub's computer at the request of a newer client (the phone):
//! `hub/update/request`, part of the attach protocol's frozen core
//! (docs/09-compatibility.md#the-frozen-core, docs/install.md#updating-from-the-phone).
//!
//! The hub decides whether a request is allowed (from an admitted Tailscale peer, for a build
//! newer than its own, one at a time) and reports progress; finding the release, downloading
//! and verifying the installer, and starting it are a [`HostInstaller`], which the `yaac`
//! binary supplies on Windows (the desktop updater's code, `yaac-app/src/updater.rs`). A hub
//! without one answers that this host does not support it. Tests supply a fake.
//!
//! Every method of a [`HostInstaller`] blocks; the hub calls them on a thread of its own,
//! never on its I/O thread.

/// A published release of the requested build, with an installer.
pub struct HostRelease {
    /// Its build, `<version>+<id>`.
    pub build: String,
    /// The installer's size in bytes.
    pub size: u64,
    /// What the installer needs to download it again (the release and manifest it read).
    pub detail: Box<dyn std::any::Any + Send>,
}

impl std::fmt::Debug for HostRelease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostRelease").field("build", &self.build).field("size", &self.size).finish_non_exhaustive()
    }
}

/// Finds, downloads, verifies and starts YAAC's installer on this computer.
pub trait HostInstaller: Send + Sync {
    /// The published release of exactly `build` (`<version>+<id>`), with an installer asset
    /// in its `manifest.json`. An error is a sentence for the user.
    fn find(&self, build: &str) -> Result<HostRelease, String>;
    /// Downloads the installer, checking its size and SHA-256 against the manifest as it
    /// arrives; `progress(done, total)` for each chunk. Nothing unverified is kept.
    fn download(&self, release: &HostRelease, progress: &mut dyn FnMut(u64, u64)) -> Result<(), String>;
    /// Starts the verified installer without a window, to stop YAAC, replace it, and start the
    /// hub and the window again when it is done. Returns once it is started; the installer
    /// then stops this hub.
    fn install(&self, release: &HostRelease) -> Result<(), String>;
}
