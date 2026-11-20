//! `yaac`: the window, or `yaac hub`, the headless hub. See [`yaac_app::run`].
//!
//! On Windows this is a GUI-subsystem program, so starting the window never opens a console
//! window. Started from a terminal (`yaac hub`, `--screenshots`, `--help`), it attaches to that
//! terminal's console so its messages still appear; output redirected to a pipe or a file is
//! left alone.

#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() -> std::process::ExitCode {
    #[cfg(windows)]
    yaac_winsys::attach_parent_console();
    yaac_app::run(&std::env::args().skip(1).collect::<Vec<_>>())
}
