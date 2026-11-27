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
//!
//! Without the `desktop` feature (the Android library) the crate is the window controller
//! only ([`view_protocol`]): the egui painting code still compiles — the controller runs egui's
//! `Context` headless and shares its state — but nothing draws it, hence the allowance.
#![cfg_attr(not(feature = "desktop"), allow(dead_code))]

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
mod preview_agent;
mod preview_controller;
mod preview_ui;
mod file_pane;
mod file_pane_ui;
mod cards;
mod commander;
mod composer;
mod dialogs;
mod display;
mod files;
mod fonts;
mod images;
mod artifacts;
mod first_run;
mod icons;
mod keyboard;
mod limits_ui;
mod logos;
mod markdown;
mod mermaid;
mod messages;
mod nav;
mod pr_ui;
mod screenshot;
mod seltext;
mod settings;
mod catalog_settings;
mod troubleshoot;
mod strip;
mod shell;
mod sidebar;
mod skills_ui;
mod terminal;
mod terminal_ui;
mod theme;
mod theme_capture;
mod custom_themes;
mod thread;
mod timeline;
mod titlebar;
mod tray;
pub mod platform;
mod updater;
mod voice;
mod voice_call;
mod voice_audio;
mod voice_capture;
#[cfg(feature = "desktop")]
mod desktop_call;
mod widgets;
mod wpf;
mod external_host;
mod tunnel_proxy;
pub mod view_protocol;

#[cfg(feature = "desktop")]
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
#[cfg(feature = "desktop")]
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
    if args.first().is_some_and(|a| a == "--wpf-controller") {
        return wpf::controller_main(args.get(1).map(String::as_str), args.get(2).map(String::as_str));
    }
    let mut demo = None;
    let mut theme = theme::ThemeChoice::System;
    let mut shots_dir = None;
    let mut theme_file = None;
    let mut capture_file = None;
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
            "--theme-file" => {
                theme_file = args.get(i + 1).map(std::path::PathBuf::from);
                i += 1;
            }
            "--capture-ui" => {
                capture_file = args.get(i + 1).map(std::path::PathBuf::from);
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
                println!("yaac [hub] | yaac quit [--wait-for <exe>] | yaac autostart on|off|status|sync [--start] | yaac remove-data | yaac --demo [fleet|fresh] [--theme dark|light|system] [--host windows|linux] [--screenshots <dir> [--only 01,02]] [--capture-ui <png> [--theme-file <json>]] [--renderer auto|dx12|vulkan|wgpu-gl|glow|software|windows-wpf]");
                return ExitCode::SUCCESS;
            }
            other => eprintln!("ignoring unknown argument {other}"),
        }
        i += 1;
    }
    if let Some(output) = capture_file {
        if demo.is_none() {
            eprintln!("YAAC: --capture-ui requires --demo");
            return ExitCode::FAILURE;
        }
        return theme_capture::capture_cli(
            &output, theme_file.as_deref(), theme, renderer_arg.as_deref(),
        );
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
    let mut choice = display::choice(&file, renderer_arg.as_deref());
    if shots.is_some() && choice == yaac_core::display::Renderer::WindowsWpf {
        choice = yaac_core::display::Renderer::Auto;
    }
    let built = display::built();
    let mut plan = yaac_core::display::plan(choice, yaac_core::display::Host::current(), built, file.crashed);
    if choice == yaac_core::display::Renderer::WindowsWpf && !built.wpf {
        plan.skipped = Some("the Windows WPF renderer is unavailable in this build".to_owned());
    }
    let size = if shots.is_some() { [1600.0, 1000.0] } else { [1440.0, 900.0] };
    let maximized = shots.is_none();
    let base = move || eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("YAAC")
            // No system title bar: the window draws its own strip (D104, `titlebar`).
            .with_decorations(false)
            .with_resizable(true)
            .with_inner_size(size)
            // Always opens maximized (owner); screenshots keep their fixed size.
            .with_maximized(maximized)
            .with_min_inner_size([960.0, 600.0])
            .with_icon(egui::IconData { rgba: app_icon::rgba(64), width: 64, height: 64 }),
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
