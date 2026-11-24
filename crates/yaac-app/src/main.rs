//! `yaac`: the window, or `yaac hub`, the headless hub. See [`yaac_app::run`].
//!
//! On Windows this is a GUI-subsystem program, so starting the window never opens a console
//! window. Started from a terminal (`yaac hub`, `--screenshots`, `--help`), it attaches to that
//! terminal's console so its messages still appear; output redirected to a pipe or a file is
//! left alone.

#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(feature = "preview")]
mod preview_demo;

fn main() -> std::process::ExitCode {
    #[cfg(feature = "preview")]
    if let Some(code) = yaac_preview::subprocess_entry() {
        return std::process::ExitCode::from(code as u8);
    }
    #[cfg(not(feature = "preview"))]
    if std::env::args_os()
        .skip(1)
        .any(|s| s == "--yaac-browser-host" || s.to_string_lossy().starts_with("--type="))
    {
        return std::process::ExitCode::FAILURE;
    }
    #[cfg(windows)]
    yaac_winsys::attach_parent_console();
    let args: Vec<_> = std::env::args().skip(1).collect();
    let code = run(&args);
    #[cfg(feature = "preview")]
    if !yaac_preview::finish_after_window_exit() {
        eprintln!("Browser host cleanup is still pending at window exit");
    }
    code
}
fn run(args: &[String]) -> std::process::ExitCode {
    if matches!(args, [demo, preview, ..] if demo == "--demo" && preview == "preview") {
        #[cfg(feature = "preview")]
        return preview_demo::run(&args[2..]);
        #[cfg(not(feature = "preview"))]
        {
            eprintln!("This build does not include the preview feature");
            return std::process::ExitCode::FAILURE;
        }
    }
    yaac_app::run(args)
}
