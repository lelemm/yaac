//! Opening a URL in the host's default browser (D119: the host only opens URLs; everything
//! else about pull requests runs on the target). Never on the UI thread: a worker thread runs
//! `ShellExecuteW` (Windows), `open` (macOS), or `xdg-open` (Linux and other Unix), and only
//! `http`/`https` URLs are passed on.

/// Whether `url` may be opened.
pub(crate) fn openable(url: &str) -> bool {
    (url.starts_with("https://") || url.starts_with("http://")) && !url.chars().any(char::is_control)
}

/// Opens `url` in the background; a failure is only logged (the user sees nothing open and
/// can use Copy link).
pub(crate) fn open(url: &str) {
    if !openable(url) {
        return;
    }
    let url = url.to_owned();
    let spawned = std::thread::Builder::new().name("open-url".into()).spawn(move || {
        if let Err(e) = open_now(&url) {
            eprintln!("could not open {url}: {e}");
        }
    });
    if let Err(e) = spawned {
        eprintln!("could not open a URL: {e}");
    }
}

#[cfg(windows)]
fn open_now(url: &str) -> std::io::Result<()> {
    yaac_winsys::open_url(url)
}

#[cfg(target_os = "macos")]
fn open_now(url: &str) -> std::io::Result<()> {
    std::process::Command::new("open").arg(url).status().map(|_| ())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn open_now(url: &str) -> std::io::Result<()> {
    use std::process::Stdio;
    // The child is waited for here, on the worker thread, so it never lingers as a zombie.
    std::process::Command::new("xdg-open").arg(url).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().map(|_| ())
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_web_urls_are_opened() {
        assert!(super::openable("https://github.com/lelemm/yaac/pull/1"));
        assert!(!super::openable("file:///etc/passwd"));
        assert!(!super::openable("calc.exe"));
        assert!(!super::openable("https://x\n--flag"));
    }
}
