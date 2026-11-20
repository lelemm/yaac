//! `yaac`: the window, and `yaac hub`.
//!
//! ```text
//! yaac                       attach to the hub (starting it if needed) and open the window
//! yaac hub                   run the headless hub (no window, GPU, or font code)
//! yaac --demo [fleet|fresh]  the window on a scripted in-memory hub
//!      [--theme dark|light|system] [--host windows|linux]
//!      [--screenshots <dir> [--only 01,02]]
//! ```
//!
//! `--host windows` pretends the host is Windows (the WSL settings are shown only there), so
//! those screens can be checked on Linux.

#[cfg(feature = "hub")]
pub mod attach;
#[cfg(feature = "hub")]
pub mod hubmode;

mod agents_ui;
mod app;
mod cards;
mod composer;
mod dialogs;
mod first_run;
mod icons;
mod markdown;
mod nav;
mod screenshot;
mod seltext;
mod settings;
mod shell;
mod sidebar;
mod theme;
mod thread;
mod widgets;

use std::process::ExitCode;

use yaac_core::demo::Scenario;

/// This build, exchanged in `hub/hello`.
pub const BUILD: &str = env!("CARGO_PKG_VERSION");

/// How the window starts.
#[derive(Debug)]
pub(crate) struct Launch {
    pub demo: Option<Scenario>,
    pub theme: theme::ThemeChoice,
    pub shots: Option<screenshot::Shots>,
    pub host_windows: bool,
    #[cfg(feature = "hub")]
    pub attach: Option<attach::AttachConfig>,
}

/// The entry point: `args` without the program name.
pub fn run(args: &[String]) -> ExitCode {
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
            "--only" => {
                only = args.get(i + 1).map(|s| s.split(',').map(str::to_owned).collect());
                i += 1;
            }
            "--help" | "-h" => {
                println!("yaac [hub] | yaac --demo [fleet|fresh] [--theme dark|light|system] [--host windows|linux] [--screenshots <dir> [--only 01,02]]");
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
    let size = if shots.is_some() { [1600.0, 1000.0] } else { [1440.0, 900.0] };
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default().with_title("YAAC").with_inner_size(size).with_min_inner_size([960.0, 600.0]),
        #[cfg(feature = "glow")]
        renderer: eframe::Renderer::Glow,
        #[cfg(all(feature = "wgpu", not(feature = "glow")))]
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    let launch = Launch {
        demo,
        theme,
        shots,
        host_windows,
        #[cfg(feature = "hub")]
        attach,
    };
    match eframe::run_native("YAAC", options, Box::new(move |cc| Ok(Box::new(app::YaacApp::new(cc, launch))))) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("yaac: {e}");
            ExitCode::FAILURE
        }
    }
}
