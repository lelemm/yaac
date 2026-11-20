//! `yaac`: the window.
//!
//! ```text
//! yaac --demo [fleet|fresh] [--theme dark|light|system] [--screenshots <dir> [--only 01,02]]
//! ```
//!
//! Without `--demo` the window starts detached: attaching to a real hub is a later package
//! (the seam is `yaac_core::link::HubLink`), as is `yaac hub`.

mod agents_ui;
mod app;
mod cards;
mod composer;
mod icons;
mod markdown;
mod screenshot;
mod settings;
mod shell;
mod sidebar;
mod theme;
mod thread;
mod widgets;

use yaac_core::demo::Scenario;

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut demo = None;
    let mut theme = theme::ThemeChoice::System;
    let mut shots_dir = None;
    let mut only: Option<Vec<String>> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--demo" => {
                demo = Some(Scenario::Fleet);
                if let Some(next) = args.get(i + 1) {
                    match next.as_str() {
                        "fresh" => {
                            demo = Some(Scenario::Fresh);
                            i += 1;
                        }
                        "fleet" => i += 1,
                        _ => {}
                    }
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
            "--screenshots" => {
                shots_dir = args.get(i + 1).map(std::path::PathBuf::from);
                i += 1;
            }
            "--only" => {
                only = args.get(i + 1).map(|s| s.split(',').map(str::to_owned).collect());
                i += 1;
            }
            "--help" | "-h" => {
                println!("yaac --demo [fleet|fresh] [--theme dark|light|system] [--screenshots <dir> [--only 01,02]]");
                return Ok(());
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
    let size = if shots.is_some() { [1600.0, 1000.0] } else { [1440.0, 900.0] };
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("YAAC")
            .with_inner_size(size)
            .with_min_inner_size([960.0, 600.0])
            .with_resizable(shots.is_none()),
        #[cfg(feature = "glow")]
        renderer: eframe::Renderer::Glow,
        #[cfg(all(feature = "wgpu", not(feature = "glow")))]
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native("YAAC", options, Box::new(move |cc| Ok(Box::new(app::YaacApp::new(cc, demo, theme, shots)))))
}
