//! Capture the real YAAC demo UI in an isolated child process.
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::ExitCode,
};
#[cfg(feature = "desktop")]
use std::{
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use crate::{app::YaacApp, theme::ThemeChoice};
use yaac_core::themes::{MAX_BYTES, Theme};

pub(crate) const WIDTH: u32 = 1280;
pub(crate) const HEIGHT: u32 = 800;
pub(crate) const TILE: [u32; 2] = [396, 216];

#[derive(Clone)]
pub(crate) enum ThemeInput {
    BuiltIn(ThemeChoice),
    Custom(Theme),
}

impl ThemeInput {
    pub(crate) fn apply(&self, app: &mut YaacApp) {
        match self {
            Self::BuiltIn(choice) => app.ui.theme = *choice,
            Self::Custom(theme) => {
                let mut catalog = yaac_core::themes::Catalog::default();
                catalog
                    .import(theme.clone(), false)
                    .expect("already validated");
                app.ui.themes = std::sync::Arc::new(catalog);
                app.ui.theme = ThemeChoice::Custom;
            }
        }
    }
}

pub(crate) struct Capture {
    _dir: tempfile::TempDir,
    pub(crate) full: Vec<u8>,
    pub(crate) tile: PathBuf,
    pub(crate) image: egui::ColorImage,
}


/// Without the desktop pieces (Android) there is no offscreen demo to start: the app draws
/// theme previews itself from the palette.
#[cfg(not(feature = "desktop"))]
pub(crate) fn generate(_input: ThemeInput, _wpf: bool) -> Result<Capture, String> {
    Err("Previews are drawn by the app".into())
}

#[cfg(feature = "desktop")]
pub(crate) fn generate(input: ThemeInput, wpf: bool) -> Result<Capture, String> {
    let dir = tempfile::Builder::new()
        .prefix("yaac-theme-")
        .tempdir()
        .map_err(display_error)?;
    let output = dir.path().join("preview.png");
    let log = dir.path().join("capture.log");
    let exe = std::env::current_exe().map_err(display_error)?;
    let mut command = Command::new(exe);
    command
        .args(["--demo", "--capture-ui"])
        .arg(&output)
        .args(["--renderer", if wpf { "windows-wpf" } else { "software" }])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(fs::File::create(&log).map_err(display_error)?));
    match input {
        ThemeInput::BuiltIn(choice) => {
            command.args(["--theme", choice.id()]);
        }
        ThemeInput::Custom(theme) => {
            let file = dir.path().join("theme.json");
            fs::write(&file, serde_json::to_vec(&theme).map_err(display_error)?)
                .map_err(display_error)?;
            command.arg("--theme-file").arg(file);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = command.spawn().map_err(display_error)?;
    let deadline = Instant::now() + Duration::from_secs(13);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) => {
                let detail = fs::read_to_string(output.with_extension("png.error.txt"))
                    .ok()
                    .or_else(|| fs::read_to_string(&log).ok())
                    .and_then(|s| {
                        s.lines()
                            .find(|line| line.starts_with("YAAC offscreen demo:"))
                            .or_else(|| s.lines().find(|line| !line.trim().is_empty()))
                            .map(str::to_owned)
                    });
                return Err(
                    detail.unwrap_or_else(|| "The offscreen YAAC demo could not render.".into())
                );
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(35)),
            result => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(result
                    .err()
                    .map_or("The offscreen YAAC demo timed out.".into(), display_error));
            }
        }
    }
    let full = fs::read(&output).map_err(display_error)?;
    let decoded = image::load_from_memory_with_format(&full, image::ImageFormat::Png)
        .map_err(display_error)?;
    let thumbnail = crop(&decoded);
    let tile = dir.path().join("tile.png");
    thumbnail.save(&tile).map_err(display_error)?;
    let rgba = thumbnail.to_rgba8();
    let image = egui::ColorImage::from_rgba_unmultiplied(
        [rgba.width() as usize, rgba.height() as usize],
        rgba.as_raw(),
    );
    Ok(Capture {
        _dir: dir,
        full,
        tile,
        image,
    })
}

fn crop(source: &image::DynamicImage) -> image::DynamicImage {
    // Include the sidebar seam and the conversation's controls in the sample.
    let width = source.width();
    let height = source.height();
    let left = width / 16;
    let top = height / 6;
    let crop_width = width * 3 / 4;
    let crop_height = (crop_width * TILE[1] / TILE[0]).min(height - top);
    source
        .crop_imm(left, top, crop_width, crop_height)
        .resize_exact(TILE[0], TILE[1], image::imageops::FilterType::Triangle)
}

fn display_error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

pub(crate) fn capture_cli(
    output: &Path,
    file: Option<&Path>,
    theme: ThemeChoice,
    renderer: Option<&str>,
) -> ExitCode {
    let run = || -> Result<(), String> {
        let input = if let Some(file) = file {
            let f = fs::File::open(file).map_err(display_error)?;
            let mut bytes = Vec::new();
            f.take(MAX_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
                .map_err(display_error)?;
            ThemeInput::Custom(Theme::parse(&bytes)?)
        } else {
            ThemeInput::BuiltIn(theme)
        };
        if renderer == Some("windows-wpf") {
            #[cfg(windows)]
            {
                return crate::wpf::capture(input, output).map_err(display_error);
            }
            #[cfg(not(windows))]
            {
                return Err("WPF is available on Windows only.".into());
            }
        }
        #[cfg(feature = "wgpu")]
        {
            pollster::block_on(egui_capture::render(input, output))
        }
        #[cfg(not(feature = "wgpu"))]
        {
            let _ = (input, output);
            Err("The offscreen demo needs the wgpu renderer.".into())
        }
    };
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("YAAC offscreen demo: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(feature = "wgpu")]
mod egui_capture;
