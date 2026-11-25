//! Shared image loading for the transcript and composer. Target reads are bounded on the
//! existing hub connection, and the window's workers decode pixels off the UI thread.
use std::collections::{HashMap, VecDeque};
use std::io::{Cursor, Read};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use eframe::egui::{self, ColorImage, TextureHandle, Vec2, vec2};
use serde_json::{Value, json};
use yaac_core::{
    attachments::{DraftRef, Source},
    client::Client,
};
use yaac_protocol::{TargetId, UploadId, UploadReadResult};

const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_EDGE: u32 = 4096;
const PREVIEWS: usize = 48;
const CHUNK_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) enum ImageSource {
    Path(String),
    Draft(DraftRef, u64),
    Target(TargetId, UploadId),
}

#[derive(Clone)]
enum ReadSource {
    Blob(String),
    Target(TargetId, UploadId),
}

struct Transfer {
    token: u64,
    job: Job,
    source: ReadSource,
    bytes: Vec<u8>,
    deadline: Instant,
}

struct Job {
    source: ImageSource,
    /// Some identifies the specific viewer so a stale reply cannot replace it.
    viewer: Option<u64>,
}

struct Decoded {
    pixels: ColorImage,
    png: Option<String>,
}

struct Finished {
    job: Job,
    result: Result<Decoded, String>,
}

struct Preview {
    pixels: Option<ColorImage>,
    texture: Option<TextureHandle>,
    wire: Value,
}

struct Viewer {
    id: u64,
    name: String,
    result: Option<Result<Decoded, String>>,
    texture: Option<TextureHandle>,
    zoom: f32,
    pan: Vec2,
    dirty: bool,
}

pub(crate) struct Images {
    ctx: egui::Context,
    tx: Sender<Finished>,
    rx: Receiver<Finished>,
    viewer: Option<Viewer>,
    serial: u64,
    previews: HashMap<ImageSource, Preview>,
    recent: VecDeque<ImageSource>,
    queue: VecDeque<Job>,
    transfer: Option<Transfer>,
    working: bool,
    wpf: bool,
}

impl Images {
    pub(crate) fn new(ctx: &egui::Context) -> Self {
        let (tx, rx) = channel();
        Self {
            ctx: ctx.clone(),
            tx,
            rx,
            viewer: None,
            serial: 0,
            previews: HashMap::new(),
            recent: VecDeque::new(),
            queue: VecDeque::new(),
            transfer: None,
            working: false,
            wpf: false,
        }
    }

    pub(crate) fn use_wpf(&mut self) {
        self.wpf = true;
    }

    pub(crate) fn open(&mut self, source: ImageSource, name: String) {
        self.serial += 1;
        self.viewer = Some(Viewer {
            id: self.serial,
            name,
            result: None,
            texture: None,
            zoom: 1.0,
            pan: Vec2::ZERO,
            dirty: true,
        });
        self.queue.retain(|job| job.viewer.is_none());
        if let Some(transfer) = self.transfer.take()
            && transfer.job.viewer.is_none()
        {
            self.queue.push_front(transfer.job);
        }
        self.queue.push_front(Job {
            source,
            viewer: Some(self.serial),
        });
        self.ctx.memory_mut(|memory| {
            if let Some(focused) = memory.focused() {
                memory.surrender_focus(focused);
            }
        });
        self.ctx.request_repaint();
    }

    pub(crate) fn is_open(&self) -> bool {
        self.viewer.is_some()
    }

    pub(crate) fn close(&mut self) {
        self.viewer = None;
        self.queue.retain(|job| job.viewer.is_none());
        if self
            .transfer
            .as_ref()
            .is_some_and(|transfer| transfer.job.viewer.is_some())
        {
            self.transfer = None;
        }
        self.ctx.request_repaint();
    }

    fn current(&self, job: &Job) -> bool {
        job.viewer
            .is_none_or(|id| self.viewer.as_ref().is_some_and(|viewer| viewer.id == id))
    }

    pub(crate) fn preview(&mut self, source: ImageSource) {
        if let Some(index) = self.recent.iter().position(|entry| entry == &source) {
            self.recent.remove(index);
            self.recent.push_back(source);
        } else if self.queue.len() < PREVIEWS {
            self.recent.push_back(source.clone());
            if self.recent.len() > PREVIEWS
                && let Some(old) = self.recent.pop_front()
            {
                self.previews.remove(&old);
                self.queue
                    .retain(|job| job.viewer.is_some() || job.source != old);
            }
            self.queue.push_back(Job {
                source,
                viewer: None,
            });
            self.ctx.request_repaint();
        }
    }

    pub(crate) fn wire_preview(&self, source: &ImageSource) -> Option<&Value> {
        self.previews.get(source).map(|preview| &preview.wire)
    }

    pub(crate) fn inline(&mut self, ui: &mut egui::Ui, source: &ImageSource, name: &str) -> bool {
        if ui.is_rect_visible(egui::Rect::from_min_size(ui.cursor().min, vec2(24.0, 22.0))) {
            self.preview(source.clone());
        }
        if let Some(preview) = self.previews.get_mut(source) {
            if let Some(pixels) = &preview.pixels {
                let texture = preview.texture.get_or_insert_with(|| {
                    self.ctx.load_texture(
                        "tool image preview",
                        pixels.clone(),
                        egui::TextureOptions::LINEAR,
                    )
                });
                let size = texture.size_vec2();
                let scale = (ui.available_width() / size.x).min(1.0);
                return ui
                    .add(egui::Image::new((texture.id(), size * scale)).sense(egui::Sense::click()))
                    .on_hover_text(format!("Open {name}"))
                    .clicked();
            }
            return ui.button(format!("Open {name}")).clicked();
        }
        ui.button(format!("Open {name}")).clicked()
    }

    pub(crate) fn pump(&mut self, client: &mut Client) {
        for finished in self.rx.try_iter().collect::<Vec<_>>() {
            self.working = false;
            self.complete(finished.job, finished.result);
        }
        for (token, reply) in std::mem::take(&mut client.image_chunks) {
            let Some(mut transfer) = self.transfer.take() else {
                continue;
            };
            if transfer.token != token {
                self.transfer = Some(transfer);
                continue;
            }
            if !self.current(&transfer.job) {
                continue;
            }
            match reply.and_then(|reply| append_chunk(reply, &mut transfer.bytes)) {
                Ok(true) => {
                    self.request(client, &mut transfer);
                    self.transfer = Some(transfer);
                }
                Ok(false) => self.decode(transfer.job, Input::Bytes(transfer.bytes)),
                Err(error) => self.complete(transfer.job, Err(error)),
            }
        }
        if self
            .transfer
            .as_ref()
            .is_some_and(|transfer| Instant::now() >= transfer.deadline)
            && let Some(transfer) = self.transfer.take()
        {
            self.complete(
                transfer.job,
                Err("Timed out while reading the image.".into()),
            );
        }
        if self.working || self.transfer.is_some() {
            return;
        }
        while let Some(job) = self.queue.pop_front() {
            if !self.current(&job) {
                continue;
            }
            let source = match &job.source {
                ImageSource::Path(path) => {
                    let path = path.clone();
                    self.decode(job, Input::Path(path));
                    return;
                }
                ImageSource::Target(target, id) => ReadSource::Target(target.clone(), id.clone()),
                ImageSource::Draft(draft, id) => {
                    let attachment = client
                        .draft(draft)
                        .and_then(|draft| draft.attachments.iter().find(|a| a.id == *id));
                    match attachment.and_then(|a| a.source.as_ref()) {
                        Some(Source::Blob { blob_id }) => ReadSource::Blob(blob_id.clone()),
                        Some(Source::Path { path }) => {
                            let path = path.clone();
                            self.decode(job, Input::Path(path));
                            return;
                        }
                        None if attachment.is_some_and(|a| {
                            matches!(a.upload, yaac_core::attachments::Upload::Storing)
                        }) =>
                        {
                            // The hub will wake the window when it finishes saving the clipboard bytes.
                            self.queue.push_front(job);
                            return;
                        }
                        None => {
                            self.complete(job, Err("The attachment bytes are unavailable.".into()));
                            continue;
                        }
                    }
                }
            };
            self.serial += 1;
            let mut transfer = Transfer {
                token: self.serial,
                job,
                source,
                bytes: Vec::new(),
                deadline: Instant::now(),
            };
            self.request(client, &mut transfer);
            self.transfer = Some(transfer);
            return;
        }
    }

    fn request(&self, client: &mut Client, transfer: &mut Transfer) {
        let offset = transfer.bytes.len() as u64;
        transfer.deadline = Instant::now() + CHUNK_TIMEOUT;
        self.ctx.request_repaint_after(CHUNK_TIMEOUT);
        match &transfer.source {
            ReadSource::Blob(id) => client.read_draft_image_chunk(transfer.token, id, offset),
            ReadSource::Target(target, id) => {
                client.read_image_chunk(transfer.token, target, id, offset);
            }
        }
    }

    fn decode(&mut self, job: Job, input: Input) {
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        let big = job.viewer.is_some();
        let wpf = self.wpf;
        let backup = Job {
            source: job.source.clone(),
            viewer: job.viewer,
        };
        self.working = std::thread::Builder::new()
            .name("yaac-image".into())
            .spawn(move || {
                let result = decode(input, big, wpf);
                let _ = tx.send(Finished { job, result });
                ctx.request_repaint();
            })
            .is_ok();
        if !self.working {
            self.complete(backup, Err("Could not start the image decoder.".into()));
        }
    }

    fn complete(&mut self, job: Job, result: Result<Decoded, String>) {
        if !self.current(&job) {
            return;
        }
        if job.viewer.is_some() {
            if let Some(viewer) = &mut self.viewer {
                viewer.result = Some(result);
                viewer.dirty = true;
            }
        } else if self.recent.contains(&job.source) {
            let (pixels, wire) = match result {
                Ok(decoded) => (
                    (!self.wpf).then_some(decoded.pixels),
                    json!({ "png": decoded.png }),
                ),
                Err(error) => (None, json!({ "error": error })),
            };
            self.previews.insert(
                job.source,
                Preview {
                    pixels,
                    texture: None,
                    wire,
                },
            );
        }
        self.ctx.request_repaint();
    }

    pub(crate) fn take_wpf(&mut self) -> Option<Value> {
        let viewer = self.viewer.as_mut().filter(|v| v.dirty)?;
        viewer.dirty = false;
        let mut message = json!({ "id": viewer.id, "name": viewer.name });
        match &mut viewer.result {
            Some(Ok(decoded)) => message["png"] = json!(decoded.png.take()),
            Some(Err(error)) => message["error"] = json!(error),
            None => {}
        }
        Some(message)
    }

    pub(crate) fn show(&mut self, ctx: &egui::Context) {
        let Some(viewer) = &mut self.viewer else {
            return;
        };
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::unique("image-viewer")).show(ctx, |ui| {
            let size = ctx.content_rect().size();
            ui.set_width((size.x - 72.0).max(240.0));
            ui.horizontal(|ui| {
                close |= ui.button("Close").clicked();
                if ui.button("−").clicked() {
                    viewer.zoom = (viewer.zoom / 1.25).max(0.25);
                }
                if ui.button("+").clicked() {
                    viewer.zoom = (viewer.zoom * 1.25).min(16.0);
                }
                if ui.button("Reset").clicked() {
                    viewer.zoom = 1.0;
                    viewer.pan = Vec2::ZERO;
                }
                ui.label(format!("{:.0}% · {}", viewer.zoom * 100.0, viewer.name));
            });
            let (rect, response) = ui.allocate_exact_size(
                vec2(ui.available_width(), (size.y - 145.0).max(140.0)),
                egui::Sense::drag(),
            );
            let painter = ui.painter_at(rect);
            painter.rect_filled(rect, 4.0, ui.visuals().extreme_bg_color);
            match &mut viewer.result {
                Some(Ok(decoded)) => {
                    let tex = viewer.texture.get_or_insert_with(|| {
                        ctx.load_texture(
                            "image viewer",
                            std::mem::take(&mut decoded.pixels),
                            egui::TextureOptions::LINEAR,
                        )
                    });
                    let native = tex.size_vec2();
                    let fit = (rect.width() / native.x)
                        .min(rect.height() / native.y)
                        .min(1.0);
                    if response.dragged() {
                        viewer.pan += response.drag_delta();
                    }
                    if response.hovered() {
                        let delta = ui.input(|i| i.smooth_scroll_delta.y);
                        if delta != 0.0 {
                            let zoom = (viewer.zoom * (delta * 0.004).exp()).clamp(0.25, 16.0);
                            if let Some(cursor) = response.hover_pos() {
                                let anchor = cursor - rect.center();
                                viewer.pan = anchor + (viewer.pan - anchor) * (zoom / viewer.zoom);
                            }
                            viewer.zoom = zoom;
                        }
                    }
                    if response.dragged() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                    } else if response.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                    }
                    let scaled = native * (fit * viewer.zoom);
                    let pan_limit = ((scaled - rect.size()) * 0.5).max(Vec2::ZERO);
                    viewer.pan = viewer.pan.clamp(-pan_limit, pan_limit);
                    let image = egui::Rect::from_center_size(rect.center() + viewer.pan, scaled);
                    painter.image(
                        tex.id(),
                        image,
                        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                }
                _ => {
                    let label = match &viewer.result {
                        Some(Err(error)) => error.as_str(),
                        _ => "Loading image…",
                    };
                    ui.put(rect, egui::Label::new(label));
                }
            }
        });
        if close || modal.should_close() {
            self.close();
        }
    }
}

pub(crate) fn append_chunk(reply: UploadReadResult, bytes: &mut Vec<u8>) -> Result<bool, String> {
    if !reply.present {
        return Err("The file is no longer available.".into());
    }
    if reply.size > MAX_BYTES as u64 {
        return Err("This file exceeds the 64 MiB preview limit.".into());
    }
    let chunk = STANDARD
        .decode(reply.data_base64)
        .map_err(|_| "The target returned invalid file data.")?;
    let total = bytes.len() + chunk.len();
    if chunk.is_empty() || total as u64 > reply.size || reply.more == (total as u64 == reply.size) {
        return Err("The target returned an incomplete file.".into());
    }
    bytes.extend_from_slice(&chunk);
    Ok(reply.more)
}

enum Input {
    Path(String),
    Bytes(Vec<u8>),
}

fn decode(input: Input, big: bool, wpf: bool) -> Result<Decoded, String> {
    let bytes = match input {
        Input::Bytes(bytes) => bytes,
        Input::Path(path) => {
            let info = std::fs::metadata(&path).map_err(|e| e.to_string())?;
            if !info.is_file() || info.len() > MAX_BYTES as u64 {
                return Err("This file is unavailable or exceeds the 64 MiB preview limit.".into());
            }
            let mut bytes = Vec::new();
            std::fs::File::open(path)
                .and_then(|file| file.take(MAX_BYTES as u64 + 1).read_to_end(&mut bytes))
                .map_err(|e| e.to_string())?;
            bytes
        }
    };
    if bytes.len() > MAX_BYTES {
        return Err("This file exceeds the 64 MiB preview limit.".into());
    }
    let mut reader = image::ImageReader::new(Cursor::new(&bytes))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(160 * 1024 * 1024);
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    reader.limits(limits);
    let Some(format) = reader.format() else {
        return Err("This file is not a supported image.".into());
    };
    if !matches!(
        format,
        image::ImageFormat::Png
            | image::ImageFormat::Jpeg
            | image::ImageFormat::Gif
            | image::ImageFormat::WebP
    ) {
        return Err("This file is not a supported image.".into());
    }
    let image = reader
        .decode()
        .map_err(|e| format!("Could not decode the image. {e}"))?;
    drop(bytes);
    let edge = if big { MAX_EDGE } else { 220 };
    let image = if image.width() > edge || image.height() > edge {
        image.resize(edge, edge, image::imageops::FilterType::Triangle)
    } else {
        image
    };
    let rgba = image.into_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    let png = if wpf {
        use image::ImageEncoder as _;
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(
                rgba.as_raw(),
                rgba.width(),
                rgba.height(),
                image::ExtendedColorType::Rgba8,
            )
            .map_err(|e| e.to_string())?;
        Some(STANDARD.encode(png))
    } else {
        None
    };
    Ok(Decoded {
        pixels: if big && wpf {
            ColorImage::default()
        } else {
            ColorImage::from_rgba_unmultiplied(size, rgba.as_raw())
        },
        png,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_reads_require_a_complete_bounded_sequence() {
        let chunk = |offset: u8, size, more| UploadReadResult {
            present: true,
            data_base64: STANDARD.encode([offset]),
            size,
            media_type: Some("image/png".into()),
            more,
        };
        let mut bytes = Vec::new();
        assert!(append_chunk(chunk(1, 2, true), &mut bytes).unwrap());
        assert!(!append_chunk(chunk(2, 2, false), &mut bytes).unwrap());
        assert_eq!(bytes, [1, 2]);
        assert!(append_chunk(chunk(3, 5, false), &mut bytes).is_err());
        assert!(append_chunk(chunk(3, MAX_BYTES as u64 + 1, true), &mut bytes).is_err());
        assert_eq!(bytes, [1, 2]);
    }

    #[test]
    fn previews_preserve_aspect_ratio_and_wpf_receives_png() {
        use image::ImageEncoder as _;
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(
                &vec![100; 400 * 200 * 4],
                400,
                200,
                image::ExtendedColorType::Rgba8,
            )
            .unwrap();
        let preview = decode(Input::Bytes(png.clone()), false, true).unwrap();
        assert_eq!(preview.pixels.size, [220, 110]);
        assert_eq!(
            &STANDARD.decode(preview.png.unwrap()).unwrap()[..8],
            b"\x89PNG\r\n\x1a\n"
        );
        let full = decode(Input::Bytes(png), true, false).unwrap();
        assert_eq!(full.pixels.size, [400, 200]);
    }
}
