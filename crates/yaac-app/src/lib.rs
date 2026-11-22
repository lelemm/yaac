//! `yaac`: the window, and `yaac hub`.
//!
//! ```text
//! yaac                       attach to the hub (starting it if needed) and open the window
//! yaac hub                   run the headless hub (no window, GPU, or font code)
//! yaac quit | autostart … | remove-data
//!                            what the Windows installer runs (`install`, docs/install.md)
//! yaac --demo [fleet|fresh]  the window on a scripted in-memory hub
//!      [--theme dark|light|system] [--host windows|linux]
//!      [--screenshots <dir> [--only 01,02]]
//! yaac ... --renderer <id>   start on this renderer once (auto, dx12, vulkan, wgpu-gl, glow,
//!                            software); see `display`
//! ```
//!
//! `--host windows` pretends the host is Windows (the WSL settings are shown only there), so
//! those screens can be checked on Linux.

#[cfg(feature = "hub")]
pub mod attach;
#[cfg(feature = "hub")]
pub mod hubmode;
#[cfg(feature = "hub")]
pub mod install;
#[cfg(feature = "hub")]
mod ssh_hub;

mod agents_ui;
mod app_icon;
mod app;
mod browser;
mod cards;
mod commander;
mod composer;
mod dialogs;
mod display;
mod files;
mod first_run;
mod icons;
mod keyboard;
mod logos;
mod markdown;
mod messages;
mod nav;
mod pr_ui;
mod screenshot;
mod seltext;
mod settings;
mod strip;
mod shell;
mod sidebar;
mod theme;
mod thread;
mod timeline;
mod titlebar;
mod tray;
mod updater;
mod widgets;

use std::process::ExitCode;

use yaac_core::demo::Scenario;

/// This build, exchanged in `hub/hello`: the version plus a hash of what was built
/// (`build.rs`), so two builds of one version are told apart and a window replaces an older
/// hub instead of attaching to it as its own build.
pub const BUILD: &str = concat!(env!("CARGO_PKG_VERSION"), "+", env!("YAAC_BUILD_ID"));

/// How the window starts.
#[derive(Debug)]
pub(crate) struct Launch {
    pub demo: Option<Scenario>,
    pub theme: theme::ThemeChoice,
    pub shots: Option<screenshot::Shots>,
    pub host_windows: bool,
    #[cfg(feature = "hub")]
    pub attach: Option<attach::AttachConfig>,
    /// The renderer in use and the stored choice (D128).
    pub display: display::DisplayState,
    /// The tray and close behaviour (D127).
    pub shell: tray::Shell,
    /// Put an icon in the tray (not for screenshots and tests).
    pub tray: bool,
    /// The in-app updater (D136): off unless this is the real window on Windows.
    pub updater: updater::Updater,
}

/// The entry point: `args` without the program name.
pub fn run(args: &[String]) -> ExitCode {
    // Started by `ssh` as SSH_ASKPASS during the hub's SSH setup: answer and exit.
    #[cfg(feature = "hub")]
    if let Some(code) = yaac_hub::ssh::askpass::client_main() {
        return code;
    }
    #[cfg(feature = "hub")]
    if let Some(code) = install::dispatch(args, BUILD) {
        return code;
    }
    if args.first().map(String::as_str) == Some("hub") {
        #[cfg(feature = "hub")]
        return hubmode::run(BUILD);
        #[cfg(not(feature = "hub"))]
        {
            eprintln!("yaac: this build has no hub (built without the `hub` feature)");
            return ExitCode::FAILURE;
        }
    }
    let mut demo = None;
    let mut theme = theme::ThemeChoice::System;
    let mut shots_dir = None;
    let mut only: Option<Vec<String>> = None;
    let mut renderer_arg: Option<String> = None;
    let mut host_windows = cfg!(windows) || std::env::var("YAAC_DEBUG_HOST").is_ok_and(|v| v == "windows");
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--demo" => {
                demo = Some(Scenario::Fleet);
                match args.get(i + 1).map(String::as_str) {
                    Some("fresh") => {
                        demo = Some(Scenario::Fresh);
                        i += 1;
                    }
                    Some("fleet") => i += 1,
                    _ => {}
                }
            }
            "--theme" => {
                theme = match args.get(i + 1).map(String::as_str) {
                    Some("dark") => theme::ThemeChoice::Dark,
                    Some("light") => theme::ThemeChoice::Light,
                    _ => theme::ThemeChoice::System,
                };
                i += 1;
            }
            "--host" => {
                host_windows = args.get(i + 1).is_some_and(|h| h == "windows");
                i += 1;
            }
            "--screenshots" => {
                shots_dir = args.get(i + 1).map(std::path::PathBuf::from);
                i += 1;
            }
            "--renderer" => {
                renderer_arg = args.get(i + 1).cloned();
                i += 1;
            }
            "--only" => {
                only = args.get(i + 1).map(|s| s.split(',').map(str::to_owned).collect());
                i += 1;
            }
            "--help" | "-h" => {
                println!("yaac [hub] | yaac quit [--wait-for <exe>] | yaac autostart on|off|status|sync [--start] | yaac remove-data | yaac --demo [fleet|fresh] [--theme dark|light|system] [--host windows|linux] [--screenshots <dir> [--only 01,02]] [--renderer auto|dx12|vulkan|wgpu-gl|glow|software]");
                return ExitCode::SUCCESS;
            }
            other => eprintln!("ignoring unknown argument {other}"),
        }
        i += 1;
    }
    let shots = shots_dir.map(|d| {
        let _ = std::fs::create_dir_all(&d);
        screenshot::Shots::new(d, only.as_deref())
    });
    if shots.is_some() && demo.is_none() {
        demo = Some(Scenario::Fleet);
    }
    #[cfg(not(feature = "hub"))]
    if demo.is_none() {
        eprintln!("yaac: this build has no hub; run it with --demo");
        return ExitCode::FAILURE;
    }
    #[cfg(feature = "hub")]
    let attach = if demo.is_none() {
        match attach::AttachConfig::from_env() {
            Ok(c) => Some(c),
            Err(e) => {
                eprintln!("yaac: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        None
    };
    // One window per user session (D127): a second `yaac` shows the running one and exits.
    let shell = tray::Shell::new();
    #[cfg(feature = "hub")]
    if let Some(c) = &attach
        && tray::claim_instance(&c.paths, &shell) == tray::Claim::AlreadyOpen
    {
        eprintln!("yaac: YAAC is already open; showing it");
        return ExitCode::SUCCESS;
    }
    // The in-app updater (D136): only the real window on Windows, in YAAC's own updates folder.
    // A verified update for a newer build that was downloaded earlier is installed now,
    // instead of starting (the installer starts YAAC again).
    #[cfg(feature = "hub")]
    let updates = attach.as_ref().filter(|_| updater::ENABLED && shots.is_none()).map(|c| c.paths.data_dir.join("updates"));
    #[cfg(not(feature = "hub"))]
    let updates: Option<std::path::PathBuf> = None;
    let updater = match updates {
        Some(dir) => match updater::at_start(&dir, BUILD) {
            updater::AtStart::Exit => return ExitCode::SUCCESS,
            updater::AtStart::Continue(notice) => updater::Updater::new(updater::Config::github(dir), BUILD, notice),
        },
        None if !updater::ENABLED => updater::Updater::off("updates are offered on Windows only"),
        None => updater::Updater::off("not checked in this window"),
    };
    // The renderer (D128): read before the window exists. Only a real start keeps a display
    // file; the demo uses one only in an explicit profile (`YAAC_CONFIG_DIR`).
    #[cfg(feature = "hub")]
    let display_path = attach.as_ref().map(|c| c.paths.config_dir.join("display.json"));
    #[cfg(not(feature = "hub"))]
    let display_path: Option<std::path::PathBuf> = None;
    let display_path = display_path.or_else(|| std::env::var_os("YAAC_CONFIG_DIR").filter(|d| !d.is_empty()).map(|d| std::path::PathBuf::from(d).join("display.json")));
    let store = display::Store::at(display_path);
    let mut file = store.load();
    file.note_unfinished_start();
    let choice = display::choice(&file, renderer_arg.as_deref());
    let plan = yaac_core::display::plan(choice, yaac_core::display::Host::current(), display::BUILT, file.crashed);
    let size = if shots.is_some() { [1600.0, 1000.0] } else { [1440.0, 900.0] };
    let maximized = shots.is_none();
    let base = move || eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("YAAC")
            // No system title bar: the window draws its own strip (D104, `titlebar`).
            .with_decorations(false)
            .with_resizable(true)
            .with_inner_size(size)
            // Always opens maximized (owner); screenshots keep their fixed size.
            .with_maximized(maximized)
            .with_min_inner_size([960.0, 600.0])
            .with_icon(eframe::egui::IconData { rgba: app_icon::rgba(64), width: 64, height: 64 }),
        // Ctrl+V reaches the window as a key, so the clipboard is read off the UI thread and a
        // copied file list or bitmap can be attached (docs/06-attachments.md); Ctrl+C and
        // Ctrl+X are turned back into egui's copy and cut in `raw_input_hook`.
        clipboard_shortcuts: false,
        // The renderer is set per attempt (`display::run_window`); a failed attempt returns
        // here so the next one can run in this process.
        run_and_return: true,
        ..Default::default()
    };
    let tray = shots.is_none();
    let launch = Launch {
        demo,
        theme,
        shots,
        host_windows,
        #[cfg(feature = "hub")]
        attach,
        display: display::DisplayState::default(),
        shell,
        tray,
        updater,
    };
    display::run_window(&base, &plan, choice, file, &store, launch)
}
