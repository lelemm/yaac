//! The window's side of attachments that touches the computer (docs/06-attachments.md): the
//! system file picker, the clipboard (a copied file list, a bitmap, text), PNG encoding of a
//! pasted bitmap, and chip thumbnails. Every one of these runs on a worker thread and reports
//! back through a channel, waking the window — the UI thread never waits on a dialog, the
//! clipboard, or a decoder.
//!
//! The window still never reads a file to upload it: a path goes to the hub, which reads and
//! hashes it. Only bytes that exist nowhere else (a clipboard bitmap, a long pasted text) are
//! encoded here and handed to the hub's draft store.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};

use egui::{self, ColorImage, TextureHandle};
use yaac_core::attachments::{BLOB_CHUNK_MAX, DraftRef};

/// A clipboard bitmap larger than this (as PNG) is refused (docs/06: 64 clipboard MiB per image).
const CLIPBOARD_IMAGE_MAX: usize = 64 * 1024 * 1024;
/// Files larger than this get no thumbnail.
const THUMB_FILE_MAX: u64 = 64 * 1024 * 1024;
/// The largest local file sent by its bytes (`Files::attach_local`).
const LOCAL_FILE_MAX: u64 = 64 * 1024 * 1024;
/// Thumbnails are decoded to at most this many pixels on the long side (shown at about half,
/// so they stay sharp on a high-density display).
const THUMB_PX: u32 = 72;

/// What a thumbnail is of.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ThumbKey {
    /// A file on this computer.
    Path(String),
    /// Bytes that exist only in the draft store (a pasted bitmap), by chip.
    Chip(DraftRef, u64),
}

enum Thumb {
    Loading,
    Ready {
        image: ColorImage,
        texture: Option<TextureHandle>,
        wire: std::cell::OnceCell<serde_json::Value>,
    },
    None,
}

/// Bytes made ready for the hub's draft store.
#[derive(Debug)]
pub(crate) struct Encoded {
    pub draft: DraftRef,
    pub name: String,
    pub media_type: String,
    pub size: u64,
    /// Base64, at most [`BLOB_CHUNK_MAX`] bytes each before encoding.
    pub chunks: Vec<String>,
    pub thumb: Option<ColorImage>,
}

/// What the clipboard held when the user pasted.
#[derive(Debug)]
pub(crate) enum Clip {
    /// A file list (copied in a file manager).
    Files(Vec<String>),
    /// A bitmap, already encoded to PNG.
    Image(Encoded),
    /// Text.
    Text(String),
    /// Something the window will not take, and why.
    Refused(String),
    /// Nothing usable.
    Nothing,
}

/// A worker's result.
#[derive(Debug)]
pub(crate) enum Outcome {
    /// The file picker closed (`None`: cancelled, or no picker is available).
    Picked(DraftRef, Option<Vec<PathBuf>>),
    /// A paste into a composer (`Some`), or into another text field (`None`: text only).
    Pasted(Option<DraftRef>, Clip),
    /// A long text, made into a file.
    Encoded(Encoded),
    /// A thumbnail decoded (or not an image).
    Thumb(ThumbKey, Option<ColorImage>),
    /// Something to tell the user, as-is.
    Notice(String),
}

/// A long text paste waiting for the user's choice.
#[derive(Clone, Debug)]
pub(crate) struct LargePaste {
    pub draft: DraftRef,
    pub text: String,
}

/// The workers' state, owned by the window.
pub(crate) struct Files {
    tx: Sender<Outcome>,
    rx: Receiver<Outcome>,
    ctx: egui::Context,
    thumbs: HashMap<ThumbKey, Thumb>,
    /// Text to paste into the focused field on the next frame (the clipboard was read off the
    /// UI thread).
    pub paste_events: Vec<egui::Event>,
    /// A long text paste, offered as a file.
    pub large_paste: Option<LargePaste>,
    /// The file picker is open.
    pub picking: bool,
    /// A notice for the composer ("Nothing on the clipboard to paste").
    pub notice: Option<String>,
}

impl std::fmt::Debug for Files {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Files").field("thumbs", &self.thumbs.len()).finish_non_exhaustive()
    }
}

impl Files {
    pub(crate) fn new(ctx: &egui::Context) -> Self {
        let (tx, rx) = channel();
        Self { tx, rx, ctx: ctx.clone(), thumbs: HashMap::new(), paste_events: Vec::new(), large_paste: None, picking: false, notice: None }
    }

    /// Run `job` on a worker thread; its outcome wakes the window.
    fn spawn(&self, name: &str, job: impl FnOnce() -> Outcome + Send + 'static) {
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        let started = std::thread::Builder::new().name(name.into()).spawn(move || {
            let _ = tx.send(job());
            ctx.request_repaint();
        });
        if started.is_err() {
            let _ = self.tx.send(Outcome::Notice("YAAC could not start a worker thread.".into()));
        }
    }

    /// Results that arrived.
    pub(crate) fn poll(&mut self) -> Vec<Outcome> {
        self.rx.try_iter().collect()
    }

    /// The attachment button: the system's file picker, several files at once.
    pub(crate) fn pick(&mut self, draft: DraftRef) {
        if self.picking {
            return;
        }
        self.picking = true;
        self.spawn("yaac-picker", move || Outcome::Picked(draft, crate::platform::pick_files("Attach files")));
    }

    /// Ctrl+V: read the clipboard. Into a composer, a file list or a bitmap becomes chips;
    /// anywhere else only text is pasted.
    pub(crate) fn paste(&self, draft: Option<DraftRef>, local_time: (i32, u32, u32, u32, u32)) {
        self.spawn("yaac-paste", move || {
            let clip = read_clipboard(draft.as_ref(), local_time);
            Outcome::Pasted(draft, clip)
        });
    }

    /// Files the hub cannot read by path (they are on this device, the hub on another
    /// machine: the Android client): each is read here, on a worker, and attached by its bytes
    /// through the hub's draft store, like a clipboard image. Images are recognised by content.
    pub(crate) fn attach_local(&self, draft: &DraftRef, paths: Vec<String>) {
        for path in paths {
            let draft = draft.clone();
            self.spawn("yaac-encode", move || match encode_file(&draft, &path) {
                Ok(enc) => Outcome::Encoded(enc),
                Err(why) => Outcome::Notice(why),
            });
        }
    }

    /// "Attach as a file" for a long text paste.
    pub(crate) fn text_as_file(&self, draft: DraftRef, text: String) {
        self.spawn("yaac-encode", move || {
            let bytes = text.into_bytes();
            Outcome::Encoded(Encoded {
                draft,
                name: "pasted-text.txt".into(),
                media_type: "text/plain".into(),
                size: bytes.len() as u64,
                chunks: chunks(&bytes),
                thumb: None,
            })
        });
    }

    /// A chip's thumbnail: decoded once, off the UI thread, at display size; `None` while it
    /// is being decoded, and for what is not an image.
    pub(crate) fn thumb(&mut self, key: &ThumbKey) -> Option<TextureHandle> {
        self.request_thumb(key);
        let Some(Thumb::Ready { image, texture, .. }) = self.thumbs.get_mut(key) else { return None };
        Some(texture.get_or_insert_with(|| {
            self.ctx.load_texture(format!("thumb-{key:?}"), image.clone(), egui::TextureOptions::LINEAR)
        }).clone())
    }

    pub(crate) fn request_thumb(&mut self, key: &ThumbKey) {
        if !self.thumbs.contains_key(key) && let ThumbKey::Path(path) = key {
            let path = path.clone();
            self.thumbs.insert(key.clone(), Thumb::Loading);
            let k = key.clone();
            self.spawn("yaac-thumb", move || Outcome::Thumb(k, decode_thumb(&path)));
        }
    }

    /// WPF consumes the same display-sized pixels directly as premultiplied BGRA.
    pub(crate) fn wire_thumb(&self, key: &ThumbKey) -> Option<&serde_json::Value> {
        let Some(Thumb::Ready { image, wire, .. }) = self.thumbs.get(key) else { return None };
        Some(wire.get_or_init(|| {
            use base64::Engine as _;
            let pixels: Vec<u8> = image.pixels.iter().flat_map(|p| [p.b(), p.g(), p.r(), p.a()]).collect();
            serde_json::json!({
                "width": image.size[0], "height": image.size[1],
                "pixels": base64::engine::general_purpose::STANDARD.encode(pixels),
            })
        }))
    }

    /// Retain only the display-sized pixels. The active renderer uses them on demand.
    pub(crate) fn set_thumb(&mut self, key: ThumbKey, image: Option<ColorImage>) {
        let t = match image {
            Some(image) => Thumb::Ready { image, texture: None, wire: std::cell::OnceCell::new() },
            None => Thumb::None,
        };
        self.thumbs.insert(key, t);
    }
}

/// Reads the clipboard: a file list first (a file manager also offers the paths as text),
/// then text, then a bitmap — the order egui itself uses for text and images, so copying from
/// an office program (which offers a picture of the text too) still pastes text.
#[cfg(feature = "desktop")]
fn read_clipboard(draft: Option<&DraftRef>, local_time: (i32, u32, u32, u32, u32)) -> Clip {
    let Ok(mut cb) = arboard::Clipboard::new() else { return Clip::Nothing };
    if draft.is_some()
        && let Ok(list) = cb.get().file_list()
    {
        let paths: Vec<String> = list.into_iter().filter(|p| p.is_absolute()).map(|p| p.to_string_lossy().into_owned()).collect();
        if !paths.is_empty() {
            return Clip::Files(paths);
        }
    }
    if let Ok(text) = cb.get_text()
        && !text.is_empty()
    {
        return Clip::Text(text.replace("\r\n", "\n"));
    }
    let Some(d) = draft else { return Clip::Nothing };
    match cb.get_image() {
        Ok(img) => {
            let (y, mo, da, h, mi) = local_time;
            let name = yaac_core::attachments::screenshot_name(y, mo, da, h, mi);
            match encode_png(img.width, img.height, &img.bytes) {
                Ok(png) if png.len() <= CLIPBOARD_IMAGE_MAX => Clip::Image(Encoded {
                    draft: d.clone(),
                    name,
                    media_type: "image/png".into(),
                    size: png.len() as u64,
                    chunks: chunks(&png),
                    thumb: thumb_of_rgba(img.width, img.height, &img.bytes),
                }),
                Ok(_) => Clip::Refused("The image on the clipboard is larger than 64 MiB.".into()),
                Err(_) => Clip::Nothing,
            }
        }
        Err(_) => Clip::Nothing,
    }
}

/// Without the desktop clipboard the embedder's hooks answer: copied files (a picture, a
/// document) as local copies, else text.
#[cfg(not(feature = "desktop"))]
fn read_clipboard(draft: Option<&DraftRef>, _local_time: (i32, u32, u32, u32, u32)) -> Clip {
    if draft.is_some()
        && let Some(files) = crate::platform::clipboard_files().filter(|f| !f.is_empty())
    {
        return Clip::Files(files.into_iter().map(|p| p.to_string_lossy().into_owned()).collect());
    }
    match crate::platform::clipboard_text() {
        Some(text) if !text.is_empty() => Clip::Text(text.replace("\r\n", "\n")),
        _ => Clip::Nothing,
    }
}

/// A local file as an attachment's bytes, at most [`LOCAL_FILE_MAX`].
fn encode_file(draft: &DraftRef, path: &str) -> Result<Encoded, String> {
    use std::io::Read as _;
    let name = std::path::Path::new(path).file_name().map_or_else(|| "file".to_owned(), |n| n.to_string_lossy().into_owned());
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .and_then(|f| f.take(LOCAL_FILE_MAX + 1).read_to_end(&mut bytes))
        .map_err(|e| format!("Could not read {name}: {e}"))?;
    if bytes.len() as u64 > LOCAL_FILE_MAX {
        return Err(format!("{name} is larger than 64 MiB."));
    }
    let image = image::guess_format(&bytes).ok().and_then(|f| match f {
        image::ImageFormat::Png => Some("image/png"),
        image::ImageFormat::Jpeg => Some("image/jpeg"),
        image::ImageFormat::Gif => Some("image/gif"),
        image::ImageFormat::WebP => Some("image/webp"),
        _ => None,
    });
    let thumb = image.and_then(|_| decode_thumb(path));
    Ok(Encoded {
        draft: draft.clone(),
        name,
        media_type: image.unwrap_or("application/octet-stream").into(),
        size: bytes.len() as u64,
        chunks: chunks(&bytes),
        thumb,
    })
}

/// RGBA8 to PNG.
pub(crate) fn encode_png(width: usize, height: usize, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let (w, h) = (u32::try_from(width).map_err(|e| e.to_string())?, u32::try_from(height).map_err(|e| e.to_string())?);
    if rgba.len() != width * height * 4 || width == 0 || height == 0 {
        return Err("not an RGBA image".into());
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().map_err(|e| e.to_string())?;
        writer.write_image_data(rgba).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

/// The draft store's chunks: standard base64 of at most [`BLOB_CHUNK_MAX`] bytes each.
pub(crate) fn chunks(bytes: &[u8]) -> Vec<String> {
    use base64::Engine as _;
    bytes.chunks(BLOB_CHUNK_MAX).map(|c| base64::engine::general_purpose::STANDARD.encode(c)).collect()
}

fn to_color_image(img: &image::RgbaImage) -> ColorImage {
    let size = [img.width() as usize, img.height() as usize];
    ColorImage::from_rgba_unmultiplied(size, img.as_raw())
}

/// At most [`THUMB_PX`] on the long side; a smaller picture is kept as it is.
fn shrink(img: &image::DynamicImage) -> ColorImage {
    if img.width() <= THUMB_PX && img.height() <= THUMB_PX {
        return to_color_image(&img.to_rgba8());
    }
    to_color_image(&img.thumbnail(THUMB_PX, THUMB_PX).to_rgba8())
}

fn thumb_of_rgba(width: usize, height: usize, rgba: &[u8]) -> Option<ColorImage> {
    let img = image::RgbaImage::from_raw(u32::try_from(width).ok()?, u32::try_from(height).ok()?, rgba.to_vec())?;
    Some(shrink(&image::DynamicImage::ImageRgba8(img)))
}

/// Decodes a file's thumbnail when its content is an image the MVP sends as one (PNG, JPEG,
/// GIF, WebP, recognised by content). The full image is decoded on this worker and dropped:
/// only the thumbnail's pixels are kept.
fn decode_thumb(path: &str) -> Option<ColorImage> {
    if std::fs::metadata(path).ok()?.len() > THUMB_FILE_MAX {
        return None;
    }
    let mut reader = image::ImageReader::open(path).ok()?.with_guessed_format().ok()?;
    if !matches!(reader.format()?, image::ImageFormat::Png | image::ImageFormat::Jpeg | image::ImageFormat::Gif | image::ImageFormat::WebP) {
        return None;
    }
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(512 * 1024 * 1024);
    reader.limits(limits);
    Some(shrink(&reader.decode().ok()?))
}

/// A picture for the demo's image chips (their files do not exist): a small diagram.
pub(crate) fn demo_thumb() -> ColorImage {
    let (w, h) = (72usize, 54usize);
    let bg = [226, 230, 236, 255];
    let ink = [60, 66, 78, 255];
    let mut px = vec![0u8; w * h * 4];
    let mut put = |x: usize, y: usize, c: [u8; 4]| {
        if x < w && y < h {
            px[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&c);
        }
    };
    for y in 0..h {
        for x in 0..w {
            put(x, y, bg);
        }
    }
    let boxes = [(6, 8, 24, 16), (42, 8, 24, 16), (24, 32, 24, 16)];
    for (bx, by, bw, bh) in boxes {
        for x in bx..bx + bw {
            put(x, by, ink);
            put(x, by + bh - 1, ink);
        }
        for y in by..by + bh {
            put(bx, y, ink);
            put(bx + bw - 1, y, ink);
        }
    }
    for t in 0..12 {
        put(18 + t, 24 + t * 8 / 12, ink);
        put(54 - t, 24 + t * 8 / 12, ink);
    }
    ColorImage::from_rgba_unmultiplied([w, h], &px)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bitmap_becomes_png_chunks_the_hub_accepts() {
        let (w, h) = (300usize, 200usize);
        let rgba: Vec<u8> = (0..w * h * 4).map(|i| u8::try_from(i * 7 % 251).unwrap_or(0)).collect();
        let png = encode_png(w, h, &rgba).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        let c = chunks(&png);
        use base64::Engine as _;
        let back: Vec<u8> = c.iter().flat_map(|s| base64::engine::general_purpose::STANDARD.decode(s).unwrap()).collect();
        assert_eq!(back, png);
        assert!(c.iter().all(|s| s.len() <= BLOB_CHUNK_MAX.div_ceil(3) * 4));
        let t = thumb_of_rgba(w, h, &rgba).unwrap();
        assert!(t.size[0] <= THUMB_PX as usize && t.size[1] <= THUMB_PX as usize);
        assert!(encode_png(2, 2, &[0; 3]).is_err());
    }

    #[test]
    fn a_local_file_is_read_into_chunks_with_its_type_by_content() {
        let dir = tempfile::tempdir().unwrap();
        let d = DraftRef::New(1);
        let png = encode_png(4, 3, &[200u8; 4 * 3 * 4]).unwrap();
        // The name says text; the content says PNG: the content wins.
        let photo = dir.path().join("photo.txt");
        std::fs::write(&photo, &png).unwrap();
        let enc = encode_file(&d, photo.to_str().unwrap()).unwrap();
        assert_eq!((enc.name.as_str(), enc.media_type.as_str(), enc.size), ("photo.txt", "image/png", png.len() as u64));
        assert!(enc.thumb.is_some());
        let notes = dir.path().join("notes.md");
        std::fs::write(&notes, "# hi").unwrap();
        let enc = encode_file(&d, notes.to_str().unwrap()).unwrap();
        assert_eq!(enc.media_type, "application/octet-stream");
        assert!(enc.thumb.is_none());
        assert!(encode_file(&d, dir.path().join("gone").to_str().unwrap()).unwrap_err().contains("gone"));
    }

    #[test]
    fn thumbnails_only_for_images_by_content() {
        let dir = tempfile::tempdir().unwrap();
        let png = encode_png(64, 32, &[200; 64 * 32 * 4]).unwrap();
        // Recognised by content, whatever the name says.
        let disguised = dir.path().join("picture.bin");
        std::fs::write(&disguised, &png).unwrap();
        let t = decode_thumb(&disguised.to_string_lossy()).expect("a thumbnail");
        assert_eq!(t.size, [64, 32]);
        let text = dir.path().join("notes.png");
        std::fs::write(&text, "not an image").unwrap();
        assert!(decode_thumb(&text.to_string_lossy()).is_none());
        assert!(decode_thumb("/nonexistent/x.png").is_none());
    }

    /// The real clipboard: needs a display, so it runs only with `YAAC_CLIPBOARD_TEST=1`
    /// (Linux under X11 or Wayland, or the Windows test executable). With
    /// `YAAC_CLIPBOARD_EXPECT_FILES=<path>;<path>` it first reads a file list another program
    /// put there (Explorer, or PowerShell's `Set-Clipboard -Path`).
    #[test]
    fn the_real_clipboard_gives_files_then_text_then_a_bitmap() {
        if std::env::var("YAAC_CLIPBOARD_TEST").as_deref() != Ok("1") {
            eprintln!("SKIPPED: set YAAC_CLIPBOARD_TEST=1 on a machine with a display");
            return;
        }
        let d = DraftRef::New(1);
        let t = (2026, 11, 20, 14, 5);
        if let Ok(expect) = std::env::var("YAAC_CLIPBOARD_EXPECT_FILES") {
            let want: Vec<String> = expect.split(';').map(str::to_owned).collect();
            match read_clipboard(Some(&d), t) {
                Clip::Files(got) => assert_eq!(got, want, "a file list copied by another program"),
                other => panic!("expected the copied files, got {other:?}"),
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let files: Vec<std::path::PathBuf> = ["error log.txt", "shot.png"].iter().map(|n| dir.path().join(n)).collect();
        for f in &files {
            std::fs::write(f, "x").unwrap();
        }
        // The clipboard's owner must stay alive while it is read (X11 serves it from here).
        let mut owner = arboard::Clipboard::new().expect("a clipboard");
        owner.set().file_list(&files).expect("set a file list");
        match read_clipboard(Some(&d), t) {
            Clip::Files(got) => assert_eq!(got, files.iter().map(|f| f.to_string_lossy().into_owned()).collect::<Vec<_>>()),
            other => panic!("expected files, got {other:?}"),
        }
        owner.set_text("plain words").unwrap();
        assert!(matches!(read_clipboard(Some(&d), t), Clip::Text(ref x) if x == "plain words"));
        let rgba: Vec<u8> = (0..20 * 10 * 4).map(|i| u8::try_from(i % 256).unwrap_or(0)).collect();
        owner.set_image(arboard::ImageData { width: 20, height: 10, bytes: rgba.into() }).unwrap();
        match read_clipboard(Some(&d), t) {
            Clip::Image(e) => {
                assert_eq!(e.name, "screenshot-2026-11-20-1405.png");
                assert_eq!(e.media_type, "image/png");
                assert!(e.thumb.is_some());
            }
            other => panic!("expected an image, got {other:?}"),
        }
        // Outside a composer only text is taken.
        assert!(matches!(read_clipboard(None, t), Clip::Nothing | Clip::Text(_)));
    }
}
