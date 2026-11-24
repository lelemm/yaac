//! Attachments in a composer (docs/06-attachments.md): the chip's state machine, the chip
//! view, what holds a message back, the input parts it becomes, and how a chip is kept in the
//! durable draft store.
//!
//! The window never reads a file itself for an upload. It names a file (`hub/upload/start`
//! with a path), or first stores bytes that exist only in its memory — a clipboard image — in
//! the hub's draft store as a *blob*, and uploads that. The hub reads, hashes, and sends; its
//! `hub/upload/progress` and `hub/upload/done` drive the chip:
//!
//! ```text
//!  clipboard bytes ──► Storing ──(blob/end)──┐
//!  a file path ───────────────────────────────┴──► Starting ──(handle)──► Running ──► Done
//!                                                                             └─────► Failed
//! ```
//!
//! An upload id is valid only on the target that issued it: a chip that is `Done` for another
//! target than its draft's counts as still uploading (the client re-uploads it).

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use yaac_protocol::{FilePart, ImagePart, InputPart, Support, TargetId, UploadId};

use crate::images::{ImageRoute, model_image_support, route};
use crate::key::ThreadKey;
use crate::state::{AppState, NewThreadDraft, SendAs};

/// The largest chunk `hub/drafts/blob/append` accepts, before base64 (the hub's
/// `BLOB_CHUNK_MAX`; repeated here because this crate cannot depend on the hub).
pub const BLOB_CHUNK_MAX: usize = 512 * 1024;

/// A text paste at least this long is offered as a file attachment instead of being inserted
/// (the composer styles at most this much markdown in place).
pub const LARGE_PASTE_BYTES: usize = 64 * 1024;

/// The hub's upload and draft-store methods (the hub's names, repeated here because this
/// crate cannot depend on it).
pub mod names {
    /// Upload a local file, or a stored blob, to a target.
    pub const UPLOAD_START: &str = "hub/upload/start";
    /// Stop an upload.
    pub const UPLOAD_CANCEL: &str = "hub/upload/cancel";
    /// Every unfinished upload.
    pub const UPLOAD_LIST: &str = "hub/upload/list";
    /// An upload's progress (notification).
    pub const UPLOAD_PROGRESS: &str = "hub/upload/progress";
    /// An upload finished, failed, or was cancelled (notification).
    pub const UPLOAD_DONE: &str = "hub/upload/done";
    /// Start storing bytes in the draft store.
    pub const BLOB_BEGIN: &str = "hub/drafts/blob/begin";
    /// The next chunk of a blob.
    pub const BLOB_APPEND: &str = "hub/drafts/blob/append";
    /// All bytes sent.
    pub const BLOB_END: &str = "hub/drafts/blob/end";
    /// Forget a blob.
    pub const BLOB_DELETE: &str = "hub/drafts/blob/delete";
    /// List drafts.
    pub const DRAFTS_LIST: &str = "hub/drafts/list";
    /// Store a draft.
    pub const DRAFTS_PUT: &str = "hub/drafts/put";
    /// Delete a draft (and its blobs).
    pub const DRAFTS_DELETE: &str = "hub/drafts/delete";
    /// The upload was cancelled.
    pub const UPLOAD_CANCELLED: &str = "upload_cancelled";
    /// The target dropped while uploading: the window asks again when it is back.
    pub const TARGET_OFFLINE: &str = "target_offline";
}

/// Which composer an attachment belongs to.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DraftRef {
    /// A started thread's composer.
    Thread(ThreadKey),
    /// A new-thread draft's composer (its local id).
    New(u64),
}

impl DraftRef {
    /// The key in the hub's draft store (also the `draft` of `hub/upload/start`).
    pub fn store_key(&self) -> String {
        match self {
            DraftRef::Thread(k) => format!("thread:{k}"),
            DraftRef::New(id) => format!("new:{id}"),
        }
    }

    /// The new-thread draft a store key names.
    pub fn new_id_of(key: &str) -> Option<u64> {
        key.strip_prefix("new:")?.parse().ok()
    }
}

/// Where the hub reads an attachment's bytes from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Source {
    /// A file on this computer (an absolute path).
    Path {
        /// The path.
        path: String,
    },
    /// A blob in the hub's durable draft store (clipboard bytes).
    #[serde(rename_all = "camelCase")]
    Blob {
        /// The blob.
        blob_id: String,
    },
}

/// Where a running upload is (the hub's `UploadPhase`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// The hub is reading the file to hash it.
    Hashing,
    /// Waiting for a free upload slot on the target.
    Queued,
    /// The target is being asked.
    Beginning,
    /// Sending bytes.
    Sending,
    /// The target is verifying them.
    Verifying,
}

impl Phase {
    /// From the wire word (an unknown word is treated as waiting).
    pub fn parse(word: &str) -> Self {
        match word {
            "hashing" => Phase::Hashing,
            "beginning" => Phase::Beginning,
            "sending" => Phase::Sending,
            "verifying" => Phase::Verifying,
            _ => Phase::Queued,
        }
    }
}

/// An attachment's upload.
#[derive(Clone, Debug, PartialEq)]
pub enum Upload {
    /// Clipboard bytes are being stored in the hub's draft store.
    Storing,
    /// `hub/upload/start` sent, or about to be; no handle yet.
    Starting,
    /// The hub is uploading it.
    Running {
        /// The hub's handle.
        handle: String,
        /// Where it is.
        phase: Phase,
        /// Bytes sent.
        sent: u64,
        /// Bytes in all (0 until known).
        total: u64,
    },
    /// Uploaded: the id to send, valid on `target` only.
    Done {
        /// The target that issued the id.
        target: TargetId,
        /// The id.
        upload_id: UploadId,
    },
    /// It failed; the hub's message, shown as-is.
    Failed {
        /// The message.
        message: String,
        /// The target dropped: it is uploaded again when the target is back.
        offline: bool,
    },
}

/// One attachment in a composer.
#[derive(Clone, Debug, PartialEq)]
pub struct DraftAttachment {
    /// Local id (unique within the window's run).
    pub id: u64,
    /// The name the agent sees.
    pub name: String,
    /// Where the bytes are; `None` when they are gone (attach it again).
    pub source: Option<Source>,
    /// The media type: the hub's (it sniffs the content), or the window's for clipboard bytes.
    pub media_type: Option<String>,
    /// Bytes, once known.
    pub size: Option<u64>,
    /// The user's choice when the model's image support is unknown.
    pub choice: Option<SendAs>,
    /// The upload.
    pub upload: Upload,
    /// Which `hub/upload/start` of this chip is current: an answer to an older one is stale.
    pub attempt: u32,
}

impl DraftAttachment {
    /// A chip for bytes the hub will read from `source`.
    pub fn new(id: u64, name: impl Into<String>, source: Option<Source>, media_type: Option<String>) -> Self {
        Self { id, name: name.into(), source, media_type, size: None, choice: None, upload: Upload::Starting, attempt: 0 }
    }

    /// The content is an image (by the hub's sniffing, or the clipboard's own type).
    pub fn is_image(&self) -> bool {
        self.media_type.as_deref().is_some_and(|m| m.starts_with("image/"))
    }

    /// Uploaded and valid on `target`.
    pub fn ready_on(&self, target: Option<&TargetId>) -> Option<&UploadId> {
        match &self.upload {
            Upload::Done { target: t, upload_id } if Some(t) == target => Some(upload_id),
            _ => None,
        }
    }

    /// The form kept in the hub's draft store (`Draft::attachments`).
    pub fn to_stored(&self) -> Value {
        let mut v = json!({ "name": self.name });
        if let Some(s) = &self.source {
            v["source"] = json!(s);
        }
        if let Some(m) = &self.media_type {
            v["mediaType"] = json!(m);
        }
        if let Some(s) = self.size {
            v["size"] = json!(s);
        }
        if let Some(c) = self.choice {
            v["choice"] = json!(match c {
                SendAs::Image => "image",
                SendAs::File => "file",
            });
        }
        match &self.upload {
            Upload::Done { target, upload_id } => {
                v["target"] = json!(target);
                v["uploadId"] = json!(upload_id);
            }
            Upload::Running { handle, .. } => v["handle"] = json!(handle),
            Upload::Failed { message, offline } => {
                v["error"] = json!(message);
                v["offline"] = json!(offline);
            }
            Upload::Storing | Upload::Starting => {}
        }
        v
    }

    /// A chip read back from the draft store. An upload that was running is `Running` with no
    /// progress (the hub may still have it, or it is started again); one that never got going
    /// is `Starting`; bytes that never reached the store are gone.
    pub fn from_stored(v: &Value, id: u64) -> Option<Self> {
        let name = v.get("name")?.as_str()?.to_owned();
        let source = v.get("source").and_then(|s| serde_json::from_value::<Source>(s.clone()).ok());
        let media_type = v.get("mediaType").and_then(Value::as_str).map(str::to_owned);
        let size = v.get("size").and_then(Value::as_u64);
        let choice = match v.get("choice").and_then(Value::as_str) {
            Some("image") => Some(SendAs::Image),
            Some("file") => Some(SendAs::File),
            _ => None,
        };
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
        let upload = match (s("target"), s("uploadId"), s("handle"), s("error")) {
            (Some(t), Some(u), _, _) => Upload::Done { target: TargetId::new(t), upload_id: UploadId::from(u.as_str()) },
            _ if source.is_none() => Upload::Failed { message: "This attachment is gone; attach it again.".into(), offline: false },
            (_, _, Some(handle), _) => Upload::Running { handle, phase: Phase::Queued, sent: 0, total: 0 },
            (_, _, _, Some(message)) => Upload::Failed { message, offline: v.get("offline").and_then(Value::as_bool).unwrap_or(false) },
            _ => Upload::Starting,
        };
        Some(Self { id, name, source, media_type, size, choice, upload, attempt: 0 })
    }
}

/// What a chip shows besides its name.
#[derive(Clone, Debug, PartialEq)]
pub enum ChipStatus {
    /// Clipboard bytes are being stored in the hub.
    Storing,
    /// Uploading: a fraction when bytes are moving, else the phase.
    Uploading {
        /// 0..=1 while sending.
        fraction: Option<f32>,
        /// Where it is.
        phase: Phase,
    },
    /// Uploaded; nothing to show.
    Ready,
    /// The hub's error, as-is; `retry` when the bytes are still there to send again.
    Failed {
        /// The message.
        message: String,
        /// "Retry" is offered.
        retry: bool,
    },
}

impl ChipStatus {
    /// Still on its way.
    pub fn pending(&self) -> bool {
        matches!(self, ChipStatus::Storing | ChipStatus::Uploading { .. })
    }
}

/// One attachment chip.
#[derive(Clone, Debug, PartialEq)]
pub struct ChipView {
    /// Attachment id.
    pub id: u64,
    /// File name.
    pub name: String,
    /// The content is an image (a thumbnail is shown).
    pub image: bool,
    /// How it will be sent, by the model selected now.
    pub route: ImageRoute,
    /// Upload state.
    pub status: ChipStatus,
}

/// The chip of one attachment, routed by the image support of the model selected **now**,
/// for a draft on `target`.
pub fn chip(a: &DraftAttachment, support: &Support, target: Option<&TargetId>) -> ChipView {
    let status = match &a.upload {
        Upload::Storing => ChipStatus::Storing,
        Upload::Starting => ChipStatus::Uploading { fraction: None, phase: Phase::Queued },
        Upload::Running { phase, sent, total, .. } => {
            #[allow(clippy::cast_precision_loss, reason = "a progress bar")]
            let fraction = (*total > 0 && matches!(phase, Phase::Sending | Phase::Beginning)).then(|| (*sent as f32 / *total as f32).clamp(0.0, 1.0));
            ChipStatus::Uploading { fraction, phase: *phase }
        }
        Upload::Done { .. } if a.ready_on(target).is_none() => ChipStatus::Uploading { fraction: None, phase: Phase::Queued },
        Upload::Done { .. } => ChipStatus::Ready,
        Upload::Failed { message, .. } => ChipStatus::Failed { message: message.clone(), retry: a.source.is_some() },
    };
    ChipView { id: a.id, name: a.name.clone(), image: a.is_image(), route: route(a.is_image(), support, a.choice), status }
}

/// Every chip of a draft.
pub fn chips(attachments: &[DraftAttachment], support: &Support, target: Option<&TargetId>) -> Vec<ChipView> {
    attachments.iter().map(|a| chip(a, support, target)).collect()
}

/// Why the message cannot be sent yet, for the send button; `None` when the chips allow it.
pub fn blocked(chips: &[ChipView]) -> Option<String> {
    let pending = chips.iter().filter(|c| c.status.pending()).count();
    if pending > 0 {
        return Some(if pending == 1 { "Waiting for 1 upload".to_owned() } else { format!("Waiting for {pending} uploads") });
    }
    if let Some(c) = chips.iter().find(|c| matches!(c.status, ChipStatus::Failed { .. })) {
        return Some(format!("{} was not uploaded · remove it or retry", c.name));
    }
    if let Some(c) = chips.iter().find(|c| !c.route.ready()) {
        return Some(format!("Choose how to send {}", c.name));
    }
    None
}

/// The input parts of a message: its text, then each attachment as an `image` or a `file`
/// part by the model selected now. `None` while anything holds it back, or when it is empty.
pub fn input_parts(text: &str, attachments: &[DraftAttachment], support: &Support, target: Option<&TargetId>) -> Option<Vec<InputPart>> {
    let mut parts = Vec::new();
    if !text.trim().is_empty() {
        parts.push(InputPart::text(text.to_owned()));
    }
    for a in attachments {
        let upload_id = a.ready_on(target)?.clone();
        let (name, media_type) = (Some(a.name.clone()), a.media_type.clone());
        match route(a.is_image(), support, a.choice) {
            ImageRoute::AsImage => parts.push(InputPart::Image(ImagePart { upload_id, name, media_type })),
            ImageRoute::AsFile | ImageRoute::AsFileCannotSee => parts.push(InputPart::File(FilePart { upload_id, name, media_type })),
            ImageRoute::Ask => return None,
        }
    }
    (!parts.is_empty()).then_some(parts)
}

/// A pasted text this long is offered as a file instead.
pub fn is_large_paste(text: &str) -> bool {
    text.len() >= LARGE_PASTE_BYTES
}

/// The name of a pasted screenshot: `screenshot-2026-11-20-1405.png`, in the host's local time.
pub fn screenshot_name(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> String {
    format!("screenshot-{year:04}-{month:02}-{day:02}-{hour:02}{minute:02}.png")
}

/// The image support a started thread's next message will be judged by: the model chosen for
/// the next message when there is one (its descriptor), else the thread's effective support.
pub fn thread_image_support(state: &AppState, key: &ThreadKey) -> Support {
    if let Some(model) = state.next_settings.get(key).and_then(|n| n.model.as_ref())
        && let Some(info) = state.provider_of(key).and_then(|p| p.models.iter().find(|m| &m.id == model))
    {
        return model_image_support(info);
    }
    state.transcripts.get(key).and_then(|t| t.effective.as_ref()).map_or(Support::Unknown, |e| e.image_input.clone())
}

/// The image support a new-thread draft's first message will be judged by: the chosen
/// model's descriptor (or the instance's default model).
pub fn draft_image_support(state: &AppState, d: &NewThreadDraft) -> Support {
    let Some(target) = d.target.as_ref().and_then(|id| state.target(id)) else { return Support::Unknown };
    let instance = d.instance.clone().or_else(|| target.providers.iter().find(|p| p.status == yaac_protocol::ProviderStatus::Ready).map(|p| p.instance.clone()));
    instance
        .and_then(|i| target.provider(&i))
        .and_then(|p| p.models.iter().find(|m| Some(&m.id) == d.model.as_ref()).or_else(|| p.models.iter().find(|m| m.is_default)))
        .map_or(Support::Unknown, model_image_support)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn done(id: u64, name: &str, media: Option<&str>, target: &str) -> DraftAttachment {
        let mut a = DraftAttachment::new(id, name, Some(Source::Path { path: format!("/tmp/{name}") }), media.map(str::to_owned));
        a.upload = Upload::Done { target: TargetId::new(target), upload_id: UploadId::from(format!("u{id}").as_str()) };
        a
    }

    #[test]
    fn chip_routing_per_support_value() {
        let t = TargetId::new("devbox");
        let img = done(1, "diagram.png", Some("image/png"), "devbox");
        let file = done(2, "trace.log", None, "devbox");
        for (support, want_img) in [
            (Support::Supported, ImageRoute::AsImage),
            (Support::Unsupported, ImageRoute::AsFileCannotSee),
            (Support::Unknown, ImageRoute::Ask),
        ] {
            let c = chips(&[img.clone(), file.clone()], &support, Some(&t));
            assert_eq!(c[0].route, want_img, "{support:?}");
            assert!(c[0].image);
            assert_eq!(c[1].route, ImageRoute::AsFile, "a file is a file whatever the model");
            assert_eq!(c[0].status, ChipStatus::Ready);
        }
        // Unknown: the user's choice decides, and until then the send button says why.
        let c = chips(std::slice::from_ref(&img), &Support::Unknown, Some(&t));
        assert_eq!(blocked(&c).as_deref(), Some("Choose how to send diagram.png"));
        let mut chosen = img.clone();
        chosen.choice = Some(SendAs::File);
        assert_eq!(blocked(&chips(&[chosen], &Support::Unknown, Some(&t))), None);
        // Unsupported is sendable (as a file).
        assert_eq!(blocked(&chips(&[img], &Support::Unsupported, Some(&t))), None);
    }

    #[test]
    fn progress_and_gating() {
        let t = TargetId::new("devbox");
        let mut a = DraftAttachment::new(1, "trace.log", Some(Source::Path { path: "/x/trace.log".into() }), None);
        assert_eq!(chip(&a, &Support::Supported, Some(&t)).status, ChipStatus::Uploading { fraction: None, phase: Phase::Queued });
        a.upload = Upload::Running { handle: "hu_1".into(), phase: Phase::Hashing, sent: 0, total: 100 };
        assert_eq!(chip(&a, &Support::Supported, Some(&t)).status, ChipStatus::Uploading { fraction: None, phase: Phase::Hashing });
        a.upload = Upload::Running { handle: "hu_1".into(), phase: Phase::Sending, sent: 62, total: 100 };
        let c = chip(&a, &Support::Supported, Some(&t));
        assert_eq!(c.status, ChipStatus::Uploading { fraction: Some(0.62), phase: Phase::Sending });
        assert_eq!(blocked(&[c.clone(), c]).as_deref(), Some("Waiting for 2 uploads"));
        assert!(input_parts("hi", std::slice::from_ref(&a), &Support::Supported, Some(&t)).is_none());
        // Done on another target: not ready here.
        let other = done(2, "x.txt", None, "staging");
        assert!(chip(&other, &Support::Supported, Some(&t)).status.pending());
        assert!(input_parts("hi", &[other], &Support::Supported, Some(&t)).is_none());
        // Failed: the hub's text, and Retry while the bytes are there.
        a.upload = Upload::Failed { message: "The file is larger than 512 MiB.".into(), offline: false };
        let c = chip(&a, &Support::Supported, Some(&t));
        assert_eq!(c.status, ChipStatus::Failed { message: "The file is larger than 512 MiB.".into(), retry: true });
        assert_eq!(blocked(&[c]).as_deref(), Some("trace.log was not uploaded · remove it or retry"));
    }

    #[test]
    fn parts_carry_upload_name_and_media_type() {
        let t = TargetId::new("devbox");
        let atts = [done(1, "shot.png", Some("image/png"), "devbox"), done(2, "error-log.txt", Some("text/plain"), "devbox")];
        let parts = input_parts("look", &atts, &Support::Supported, Some(&t)).unwrap();
        let v = serde_json::to_value(&parts).unwrap();
        assert_eq!(v[0]["type"], "text");
        assert_eq!(v[1], json!({ "type": "image", "uploadId": "u1", "name": "shot.png", "mediaType": "image/png" }));
        assert_eq!(v[2], json!({ "type": "file", "uploadId": "u2", "name": "error-log.txt", "mediaType": "text/plain" }));
        // The same image to a model that cannot see: a file part.
        let parts = input_parts("", &atts[..1], &Support::Unsupported, Some(&t)).unwrap();
        assert!(matches!(parts[0], InputPart::File(_)));
        // Nothing at all: nothing to send.
        assert!(input_parts("  ", &[], &Support::Supported, Some(&t)).is_none());
    }

    #[test]
    fn stored_form_round_trips() {
        let mut a = done(7, "diagram.png", Some("image/png"), "devbox");
        a.source = Some(Source::Blob { blob_id: "b_1".into() });
        a.size = Some(1234);
        a.choice = Some(SendAs::Image);
        let back = DraftAttachment::from_stored(&a.to_stored(), 7).unwrap();
        assert_eq!(back, a);
        // A running upload comes back running (to be checked against `hub/upload/list`).
        let mut r = a.clone();
        r.upload = Upload::Running { handle: "hu_9".into(), phase: Phase::Sending, sent: 5, total: 9 };
        assert_eq!(DraftAttachment::from_stored(&r.to_stored(), 7).unwrap().upload, Upload::Running { handle: "hu_9".into(), phase: Phase::Queued, sent: 0, total: 0 });
        // One that never started: started again.
        let mut s = a;
        s.upload = Upload::Starting;
        assert_eq!(DraftAttachment::from_stored(&s.to_stored(), 7).unwrap().upload, Upload::Starting);
        // Clipboard bytes that never reached the store are gone.
        let mut gone = s;
        gone.source = None;
        assert!(matches!(DraftAttachment::from_stored(&gone.to_stored(), 7).unwrap().upload, Upload::Failed { .. }));
        assert_eq!(json!(Source::Path { path: "/a".into() }), json!({ "kind": "path", "path": "/a" }));
        assert_eq!(json!(Source::Blob { blob_id: "b".into() }), json!({ "kind": "blob", "blobId": "b" }));
    }

    #[test]
    fn keys_names_and_large_pastes() {
        assert_eq!(DraftRef::Thread(ThreadKey::new("devbox", "t1")).store_key(), "thread:devbox/t1");
        assert_eq!(DraftRef::New(3).store_key(), "new:3");
        assert_eq!(DraftRef::new_id_of("new:3"), Some(3));
        assert_eq!(DraftRef::new_id_of("thread:x/y"), None);
        assert_eq!(screenshot_name(2026, 11, 20, 14, 5), "screenshot-2026-11-20-1405.png");
        assert!(!is_large_paste("short"));
        assert!(is_large_paste(&"x".repeat(LARGE_PASTE_BYTES)));
    }
}
