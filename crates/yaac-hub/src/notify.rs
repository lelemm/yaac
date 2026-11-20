//! Desktop notifications from the hub: a thread needs the user and no window is attached
//! (docs/10-agent-mesh.md#a-worker-that-is-blocked-must-be-seen).
//!
//! Off unless the configuration supplies a notifier ([`HubConfig::from_env`] does; tests and
//! [`HubConfig::new`] do not). Each platform is one function: Linux uses `notify-send` when it
//! is on `PATH` and stays silent otherwise; Windows is not implemented yet.
//!
//! [`HubConfig::from_env`]: crate::HubConfig::from_env
//! [`HubConfig::new`]: crate::HubConfig::new

use std::path::PathBuf;

/// Shows a desktop notification. Must not block: the hub calls it on its I/O thread.
pub trait Notifier: Send + Sync {
    /// Shows one notification.
    fn notify(&self, title: &str, body: &str);
}

/// The platform's notifications.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemNotifier;

impl Notifier for SystemNotifier {
    fn notify(&self, title: &str, body: &str) {
        #[cfg(target_os = "linux")]
        linux(title, body);
        #[cfg(windows)]
        windows(title, body);
        #[cfg(not(any(target_os = "linux", windows)))]
        let _ = (title, body);
    }
}

/// An executable on `PATH`.
fn on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(name)).find(|p| p.is_file())
}

#[cfg(target_os = "linux")]
fn linux(title: &str, body: &str) {
    let Some(exe) = on_path("notify-send") else { return };
    let (title, body) = (title.to_owned(), body.to_owned());
    // Spawned and reaped off the I/O thread; failures are silent (a notification is best effort).
    std::thread::spawn(move || {
        let _ = std::process::Command::new(exe)
            .args(["--app-name=YAAC", "--urgency=normal", "--", &title, &body])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    });
}

#[cfg(windows)]
fn windows(_title: &str, _body: &str) {
    // A toast through the Windows notification API: a later package.
    let _ = on_path("powershell.exe");
}
