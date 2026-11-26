//! Host-side diagram preview. The WPF view runs the same private Node entry point.
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::Deserialize;

pub(crate) fn language(info: &str) -> bool {
    info.split_whitespace()
        .next()
        .is_some_and(|s| s.eq_ignore_ascii_case("mermaid"))
}

struct Render {
    texture: egui::TextureHandle,
    png: Arc<[u8]>,
    svg: Arc<[u8]>,
}

struct Preview {
    ready_after: Instant,
    result: OnceLock<Result<Render, String>>,
    started: AtomicBool,
}

#[derive(Default)]
struct NewPreview;
impl egui::cache::ComputerMut<(&str, bool), Arc<Preview>> for NewPreview {
    fn compute(&mut self, _: (&str, bool)) -> Arc<Preview> {
        Arc::new(Preview {
            ready_after: Instant::now() + Duration::from_millis(350),
            started: AtomicBool::new(false),
            result: OnceLock::new(),
        })
    }
}

fn render(ctx: &egui::Context, source: &str, dark: bool) -> Result<Render, String> {
    let runtime = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .parent()
        .ok_or("Missing executable directory")?
        .join("runtime");
    let script = runtime.join("mermaid/render.cjs");
    let (node, script) = if script.is_file() {
        (
            runtime.join(if cfg!(windows) { "node.exe" } else { "node" }),
            script,
        )
    } else {
        (
            PathBuf::from("node"),
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tools/mermaid/render.cjs"),
        )
    };
    let mut cmd = Command::new(node);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    let mut child = cmd
        .arg(script)
        .arg(if dark { "dark" } else { "light" })
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let write = child
        .stdin
        .take()
        .ok_or("Missing diagram input")?
        .write_all(source.as_bytes());
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    write.map_err(|e| e.to_string())?;
    #[derive(Deserialize)]
    struct Images {
        svg: String,
        png: String,
    }
    let images: Images = serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())?;
    let png = STANDARD.decode(images.png).map_err(|e| e.to_string())?;
    let svg = STANDARD.decode(images.svg).map_err(|e| e.to_string())?;
    let img = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
        .map_err(|e| e.to_string())?
        .into_rgba8();
    let pixels = egui::ColorImage::from_rgba_unmultiplied(
        [img.width() as usize, img.height() as usize],
        &img,
    );
    Ok(Render {
        texture: ctx.load_texture("Mermaid", pixels, egui::TextureOptions::LINEAR),
        png: png.into(),
        svg: svg.into(),
    })
}

fn write_export(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("Missing destination directory"))?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(bytes)?;
    file.persist(path)?;
    Ok(())
}

fn save(bytes: Arc<[u8]>, ext: &'static str) {
    std::thread::spawn(move || {
        if let Err(e) = crate::platform::save_with(
            &format!("diagram.{ext}"),
            Some(crate::platform::Filter { name: ext, extensions: &[ext] }),
            |path| write_export(path, &bytes),
        ) {
            crate::platform::message("Save diagram", &e.to_string());
        }
    });
}

pub(crate) fn show(ui: &mut egui::Ui, source: &str, dark: bool) {
    static BUSY: AtomicBool = AtomicBool::new(false);
    type Cache = egui::cache::FrameCache<Arc<Preview>, NewPreview>;
    let preview = ui
        .ctx()
        .memory_mut(|memory| Arc::clone(memory.caches.cache::<Cache>().get((source, dark))));
    if source.len() > 65536 {
        preview
            .result
            .get_or_init(|| Err("Mermaid source exceeds 64 KiB.".into()));
    } else if !preview.started.load(Ordering::Acquire)
        && Instant::now() >= preview.ready_after
        && !BUSY.swap(true, Ordering::AcqRel)
    {
        preview.started.store(true, Ordering::Release);
        let ctx = ui.ctx().clone();
        let source = source.to_owned();
        let task = Arc::clone(&preview);
        std::thread::spawn(move || {
            let _ = task.result.set(render(&ctx, &source, dark));
            BUSY.store(false, Ordering::Release);
            ctx.request_repaint();
        });
    }
    match preview.result.get() {
        Some(Ok(image)) => {
            let size = image.texture.size_vec2();
            let scale = (ui.available_width() / size.x).min(1.0);
            ui.add(egui::Image::new((image.texture.id(), size * scale)));
        }
        Some(Err(error)) => {
            ui.label(format!("Mermaid preview unavailable · {error}"));
            ui.monospace(source);
        }
        None => {
            ui.label("Rendering diagram…");
            if !preview.started.load(Ordering::Acquire) && Instant::now() < preview.ready_after {
                ui.ctx().request_repaint_after(
                    preview
                        .ready_after
                        .saturating_duration_since(Instant::now()),
                );
            }
        }
    }
    ui.horizontal_wrapped(|ui| {
        if let Some(Ok(image)) = preview.result.get() {
            if ui.button("Save PNG").clicked() {
                save(image.png.clone(), "png");
            }
            if ui.button("Save SVG").clicked() {
                save(image.svg.clone(), "svg");
            }
        }
        if ui.button("Copy source").clicked() {
            ui.ctx().copy_text(source.to_owned());
        }
        if ui.button("Save source").clicked() {
            save(Arc::from(source.as_bytes()), "mmd");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mermaid_export_replaces_the_selected_file() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("diagram.mmd");
        std::fs::write(&path, b"old contents").unwrap();
        let source = "graph LR\n A[東京] --> B[été]\n";
        write_export(&path, source.as_bytes()).unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), source);
    }

    #[test]
    fn mermaid_fence_preserves_source() {
        let input = "~~~MeRmAiD\nflowchart LR\n A --> B\n\n~~~";
        let [crate::markdown::Block::Code { lang, text }] = &crate::markdown::parse(input)[..]
        else {
            panic!("expected fence")
        };
        assert!(language(lang));
        assert_eq!(text, "flowchart LR\n A --> B\n\n");
        assert!(!language("mermaid-js"));
    }

    #[test]
    #[ignore = "requires the host Mermaid runtime and Chromium"]
    fn mermaid_preview_loads_in_egui() {
        let ctx = egui::Context::default();
        let source = "flowchart LR\n A[\"Ação<br/>München\"] -->|été| B[\"東京\"]";
        let start = Instant::now();
        loop {
            let mut ready = false;
            let mut frame = ctx.run_ui(egui::RawInput::default(), |ui| {
                show(ui, source, true);
                type Cache = egui::cache::FrameCache<Arc<Preview>, NewPreview>;
                ctx.memory_mut(|memory| {
                    let preview = memory.caches.cache::<Cache>().get((source, true));
                    if let Some(result) = preview.result.get() {
                        let image = result.as_ref().unwrap_or_else(|e| panic!("{e}"));
                        assert!(image.texture.size()[0] > 100);
                        assert!(image.png.starts_with(b"\x89PNG"));
                        let svg = std::str::from_utf8(&image.svg).unwrap();
                        assert!(
                            svg.contains("<path")
                                && !svg.contains("<text")
                                && !svg.contains("foreignObject")
                        );
                        ready = true;
                    }
                });
            });
            frame.textures_delta.clear();
            if ready {
                break;
            }
            assert!(
                start.elapsed() < Duration::from_secs(40),
                "preview did not finish"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}
