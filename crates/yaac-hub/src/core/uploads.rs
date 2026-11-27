//! Uploads from the window to a target (docs/06-attachments.md#the-flow), on the target's
//! one connection.
//!
//! The window names a file (or a draft blob); the hub resolves it, then a pool worker reads it
//! once to compute its full SHA-256 and sniff its media type. With a free slot on the target
//! the hub sends `upload/begin`; when the target already has the content it answers
//! `exists`, and no bytes are sent. Otherwise pool workers read the file in bounded chunks,
//! only as fast as the connection drains them, and the session's scheduler sends them as
//! **bulk** data frames under the target's credit. The core hands the writer one bulk frame at
//! a time ([`Core::flush`]), so a Stop or an approval answer waits behind at most one chunk.
//! When the last byte has left, `upload/end` asks the target to verify the digest.
//!
//! ```text
//! start → Hashing (pool) → Queued → Beginning → Sending (pool reads ↔ credit) → Verifying → done
//!                                     └─ exists ─────────────────────────────────────────► done (skipped)
//! ```
//!
//! Every handle ends in exactly one `hub/upload/done`. A dropped connection fails the uploads
//! on it (`target_offline`); the window asks again, as it does when a draft's target changes.

use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use yaac_protocol::methods::{self, UploadBegin, UploadEnd};
use yaac_protocol::rpc::{ErrorCode, ErrorObject};
use yaac_protocol::{Sha256Hex, StreamId, TargetId, UploadBeginParams, UploadCancelParams, UploadEndParams, UploadId};

use super::{Core, CoreEvent, Pending, err, hub_err, to_value};
use crate::drafts::{hex, human};
use crate::protocol::{
    self as ext, UploadDoneParams, UploadListResult, UploadPhase, UploadProgressParams, UploadStartParams, UploadStartResult,
};

/// Upload limits (docs/06-attachments.md#the-upload-folder). The target enforces its own as
/// well; these make the hub fail early, with a message to show as-is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UploadLimits {
    /// Largest file.
    pub max_file: u64,
    /// Bytes of all unfinished uploads together.
    pub max_total: u64,
    /// Uploads sending to one target at once; more wait in the hub's queue.
    pub max_active: usize,
    /// Unfinished uploads the hub accepts at all (hashing, queued, sending).
    pub max_pending: usize,
    /// Largest clipboard blob kept in the draft store.
    pub max_blob: u64,
}

impl Default for UploadLimits {
    fn default() -> Self {
        Self {
            max_file: 512 * 1024 * 1024,
            max_total: 2 * 1024 * 1024 * 1024,
            max_active: 4,
            max_pending: 64,
            max_blob: 64 * 1024 * 1024,
        }
    }
}

/// Bytes a pool worker reads per job.
const READ_CHUNK: u64 = 256 * 1024;
/// A new read is started once fewer than this many bytes wait in the scheduler.
const READ_LOW: u64 = 256 * 1024;
/// Progress is reported at most this often (and at every phase change).
const PROGRESS_EVERY: Duration = Duration::from_millis(100);

/// What a pool worker found when it hashed a file.
#[derive(Debug)]
pub(crate) struct Hashed {
    size: u64,
    sha256: String,
    media_type: Option<String>,
}

/// One upload the hub is carrying out.
#[derive(Debug)]
pub(super) struct Upload {
    seq: u64,
    handle: String,
    target: TargetId,
    draft: Option<String>,
    pub(super) blob_id: Option<String>,
    path: Option<PathBuf>,
    name: String,
    media_type: Option<String>,
    /// The window chose the media type; sniffing does not override it.
    media_type_given: bool,
    size: u64,
    sha256: Option<Sha256Hex>,
    phase: UploadPhase,
    /// Cancel requested while a request to the target is in flight.
    cancelled: bool,
    /// Tells a pool worker hashing this file to stop.
    stop: Arc<AtomicBool>,
    generation: u64,
    upload_id: Option<UploadId>,
    stream: Option<StreamId>,
    /// Bytes read from the file and handed to the session.
    read_offset: u64,
    reading: bool,
    /// Every byte is queued and the stream is closed.
    closed: bool,
    last_progress: Option<Instant>,
}

fn unreadable(path: &Path, e: &std::io::Error) -> ErrorObject {
    hub_err(ext::codes::FILE_UNREADABLE, format!("cannot read {}: {e}", path.display()))
}

fn cancelled() -> ErrorObject {
    hub_err(ext::codes::UPLOAD_CANCELLED, "the upload was cancelled")
}

/// Media type by content (magic bytes), as `yaacd` sniffs it.
pub(crate) fn sniff(head: &[u8]) -> Option<&'static str> {
    let at = |offset: usize, sig: &[u8]| head.len() >= offset + sig.len() && &head[offset..offset + sig.len()] == sig;
    if at(0, &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        Some("image/png")
    } else if at(0, &[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if at(0, b"GIF8") {
        Some("image/gif")
    } else if at(0, b"RIFF") && at(8, b"WEBP") {
        Some("image/webp")
    } else if at(0, b"%PDF") {
        Some("application/pdf")
    } else {
        None
    }
}

/// Reads a whole file once: size, full SHA-256, sniffed media type. Runs on a pool worker.
fn hash_file(path: &Path, max: u64, stop: &AtomicBool) -> Result<Hashed, ErrorObject> {
    let meta = std::fs::metadata(path).map_err(|e| unreadable(path, &e))?;
    if !meta.is_file() {
        return Err(hub_err(ext::codes::FILE_UNREADABLE, format!("{} is not a file", path.display())));
    }
    let too_big = |size: u64| {
        err(ErrorCode::LimitExceeded, format!("files are limited to {}; {} is {}", human(max), display_name(path), human(size)))
    };
    if meta.len() > max {
        return Err(too_big(meta.len()));
    }
    let mut file = std::fs::File::open(path).map_err(|e| unreadable(path, &e))?;
    let mut hash = Sha256::new();
    let mut head = Vec::with_capacity(16);
    let mut buf = vec![0u8; 1024 * 1024];
    let mut size = 0u64;
    loop {
        if stop.load(Ordering::Relaxed) {
            return Err(cancelled());
        }
        let n = match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(unreadable(path, &e)),
        };
        if head.len() < 16 {
            let take = (16 - head.len()).min(n);
            head.extend_from_slice(&buf[..take]);
        }
        hash.update(&buf[..n]);
        size += n as u64;
        if size > max {
            return Err(too_big(size));
        }
    }
    Ok(Hashed { size, sha256: hex(&hash.finalize()), media_type: sniff(&head).map(str::to_owned) })
}

/// Reads up to `len` bytes at `offset`. Runs on a pool worker.
fn read_chunk(path: &Path, offset: u64, len: u64) -> Result<Vec<u8>, ErrorObject> {
    let mut file = std::fs::File::open(path).map_err(|e| unreadable(path, &e))?;
    file.seek(SeekFrom::Start(offset)).map_err(|e| unreadable(path, &e))?;
    let mut out = Vec::with_capacity(usize::try_from(len).unwrap_or(0));
    file.take(len).read_to_end(&mut out).map_err(|e| unreadable(path, &e))?;
    Ok(out)
}

fn display_name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

fn changed(name: &str) -> ErrorObject {
    hub_err(ext::codes::FILE_UNREADABLE, format!("{name} changed while it was being uploaded; attach it again"))
}

impl Core {
    // ── Requests from a window ───────────────────────────────────────────────────────

    pub(super) fn upload_start(&mut self, p: UploadStartParams) -> Result<Value, ErrorObject> {
        let Some(t) = self.targets.get(&p.target) else {
            return Err(err(ErrorCode::TargetUnknown, format!("no target {}", p.target)));
        };
        if !t.conn.as_ref().is_some_and(|c| c.ready) {
            return Err(err(ErrorCode::TargetOffline, format!("{} is not connected; the attachment is uploaded when it is", t.def.name)));
        }
        let path = match (&p.path, &p.blob_id) {
            (Some(path), None) => {
                let path = PathBuf::from(path);
                if !path.is_absolute() {
                    return Err(err(ErrorCode::InvalidParams, "path must be absolute"));
                }
                Some(path)
            }
            (None, Some(_)) => None,
            _ => return Err(err(ErrorCode::InvalidParams, "give exactly one of path and blobId")),
        };
        let limits = self.cfg.uploads;
        if self.uploads.len() >= limits.max_pending {
            return Err(err(
                ErrorCode::LimitExceeded,
                format!("at most {} attachments can be uploading at once; wait for some to finish", limits.max_pending),
            ));
        }
        self.upload_seq += 1;
        let seq = self.upload_seq;
        let handle = format!("hu_{seq}");
        let name = p.name.clone().filter(|n| !n.trim().is_empty()).or_else(|| path.as_deref().map(display_name)).unwrap_or_default();
        let upload = Upload {
            seq,
            handle: handle.clone(),
            target: p.target.clone(),
            draft: p.draft.clone(),
            blob_id: p.blob_id.clone(),
            path: path.clone(),
            name,
            media_type_given: p.media_type.is_some(),
            media_type: p.media_type.clone(),
            size: 0,
            sha256: None,
            phase: UploadPhase::Hashing,
            cancelled: false,
            stop: Arc::new(AtomicBool::new(false)),
            generation: 0,
            upload_id: None,
            stream: None,
            read_offset: 0,
            reading: false,
            closed: false,
            last_progress: None,
        };
        self.uploads.insert(handle.clone(), upload);
        self.upload_progress(&handle, true);
        match (path, p.blob_id) {
            (Some(_), _) => self.upload_hash(&handle),
            (None, Some(blob)) => {
                let h = handle.clone();
                self.worker.submit(move |s| {
                    let result = match s.drafts.blob(&blob) {
                        Ok(Some((b, file))) => Ok((file, b.name, b.media_type)),
                        Ok(None) => Err(hub_err(ext::codes::BLOB_NOT_FOUND, format!("no stored attachment {blob}; attach it again"))),
                        Err(e) => Err(err(ErrorCode::InternalError, e.to_string())),
                    };
                    Some(CoreEvent::UploadSource { handle: h, result })
                });
            }
            (None, None) => {}
        }
        Ok(to_value(&UploadStartResult { handle }))
    }

    pub(super) fn upload_cancel(&mut self, handle: &str) -> Value {
        let Some(u) = self.uploads.get_mut(handle) else { return json!({}) };
        u.stop.store(true, Ordering::Relaxed);
        match u.phase {
            UploadPhase::Hashing | UploadPhase::Queued => self.upload_finish(handle, Err(cancelled())),
            UploadPhase::Beginning | UploadPhase::Verifying => u.cancelled = true,
            UploadPhase::Sending if !u.cancelled => {
                u.cancelled = true;
                let (target, stream, upload_id) = (u.target.clone(), u.stream, u.upload_id.clone());
                if let Some(conn) = self.targets.get_mut(&target).and_then(super::Target::ready_conn)
                    && let Some(stream) = stream
                {
                    conn.session.cancel_stream(stream);
                }
                let sent = upload_id.is_some_and(|upload_id| {
                    self.send::<methods::UploadCancel>(&target, &UploadCancelParams { upload_id }, Pending::UploadCancel { handle: handle.to_owned() })
                });
                if sent {
                    self.flush(&target);
                } else {
                    self.upload_finish(handle, Err(cancelled()));
                }
            }
            UploadPhase::Sending => {}
        }
        json!({})
    }

    pub(super) fn upload_list(&self) -> Value {
        let mut uploads: Vec<&Upload> = self.uploads.values().collect();
        uploads.sort_by_key(|u| u.seq);
        to_value(&UploadListResult { uploads: uploads.into_iter().map(|u| self.progress_of(u)).collect() })
    }

    pub(super) fn uploads_active(&self) -> usize {
        self.uploads.len()
    }

    // ── Pool and store completions ───────────────────────────────────────────────────

    pub(super) fn on_upload_source(&mut self, handle: &str, result: Result<(PathBuf, String, Option<String>), ErrorObject>) {
        let Some(u) = self.uploads.get_mut(handle) else { return };
        match result {
            Ok((path, name, media_type)) => {
                u.path = Some(path);
                if u.name.is_empty() {
                    u.name = name;
                }
                if !u.media_type_given {
                    u.media_type = media_type;
                }
                self.upload_hash(handle);
            }
            Err(e) => self.upload_finish(handle, Err(e)),
        }
    }

    fn upload_hash(&mut self, handle: &str) {
        let Some(u) = self.uploads.get(handle) else { return };
        let Some(path) = u.path.clone() else { return };
        let (h, stop, max) = (handle.to_owned(), u.stop.clone(), self.cfg.uploads.max_file);
        self.pool.submit(move || {
            let result = hash_file(&path, max, &stop);
            Some(CoreEvent::UploadHashed { handle: h, result })
        });
    }

    pub(super) fn on_upload_hashed(&mut self, handle: &str, result: Result<Hashed, ErrorObject>) {
        let max_total = self.cfg.uploads.max_total;
        let others: u64 = self.uploads.values().filter(|u| u.handle != handle).map(|u| u.size).sum();
        let Some(u) = self.uploads.get_mut(handle) else { return };
        let hashed = match result {
            Ok(h) => h,
            Err(e) => return self.upload_finish(handle, Err(e)),
        };
        u.size = hashed.size;
        u.sha256 = Some(Sha256Hex::new(hashed.sha256));
        if !u.media_type_given && hashed.media_type.is_some() {
            u.media_type = hashed.media_type;
        }
        if others + hashed.size > max_total {
            let message = format!(
                "attachments being uploaded are limited to {} together; wait for some to finish before adding {}",
                human(max_total),
                u.name
            );
            return self.upload_finish(handle, Err(err(ErrorCode::LimitExceeded, message)));
        }
        u.phase = UploadPhase::Queued;
        let target = u.target.clone();
        self.upload_progress(handle, true);
        self.upload_admit(&target);
    }

    pub(super) fn on_upload_chunk(&mut self, handle: &str, offset: u64, result: Result<Vec<u8>, ErrorObject>) {
        let Some(u) = self.uploads.get_mut(handle) else { return };
        if u.phase != UploadPhase::Sending || u.cancelled || offset != u.read_offset {
            return;
        }
        u.reading = false;
        let bytes = match result {
            Ok(bytes) if bytes.is_empty() && u.read_offset < u.size => Err(changed(&u.name)),
            other => other,
        };
        let (target, stream) = (u.target.clone(), u.stream);
        let bytes = match bytes {
            Ok(bytes) => bytes,
            Err(e) => return self.upload_abort(handle, e),
        };
        u.read_offset += bytes.len() as u64;
        let Some(stream) = stream else { return };
        let queued = self.targets.get_mut(&target).and_then(super::Target::ready_conn).map(|c| c.session.send_data(stream, bytes));
        match queued {
            Some(Ok(())) => self.flush(&target),
            Some(Err(e)) => self.upload_abort(handle, err(ErrorCode::InternalError, e.to_string())),
            None => {} // the connection is going; on_closed fails the upload
        }
    }

    // ── Target answers ───────────────────────────────────────────────────────────────

    pub(super) fn on_upload_begun(&mut self, id: &TargetId, handle: &str, outcome: Result<Value, ErrorObject>) {
        let result = outcome.and_then(|w| self.decode_typed::<UploadBegin>(id, w));
        let Some(u) = self.uploads.get_mut(handle) else {
            // Finished meanwhile (the connection dropped and came back): let the target go.
            if let Ok(r) = result
                && !r.exists
            {
                self.send::<methods::UploadCancel>(id, &UploadCancelParams { upload_id: r.upload_id }, Pending::Ignore);
            }
            return;
        };
        let r = match result {
            Ok(r) => r,
            Err(e) => return self.upload_finish(handle, Err(e)),
        };
        u.upload_id = Some(r.upload_id.clone());
        if u.cancelled {
            if !r.exists {
                self.send::<methods::UploadCancel>(id, &UploadCancelParams { upload_id: r.upload_id }, Pending::Ignore);
            }
            return self.upload_finish(handle, Err(cancelled()));
        }
        let stream = match (r.exists, r.stream) {
            (true, _) => return self.upload_finish(handle, Ok(true)),
            (false, Some(stream)) => stream,
            (false, None) => return self.upload_abort(handle, err(ErrorCode::InternalError, "the target gave no data stream for the upload")),
        };
        u.stream = Some(stream);
        u.phase = UploadPhase::Sending;
        let opened = self.targets.get_mut(id).and_then(super::Target::ready_conn).map(|c| c.session.open_stream(stream, r.credit));
        if let Some(Err(e)) = opened {
            return self.upload_abort(handle, err(ErrorCode::InternalError, e.to_string()));
        }
        self.upload_progress(handle, true);
        self.flush(id);
    }

    pub(super) fn on_upload_ended(&mut self, id: &TargetId, handle: &str, outcome: Result<Value, ErrorObject>) {
        let result = outcome.and_then(|w| self.decode_typed::<UploadEnd>(id, w));
        let Some(u) = self.uploads.get(handle) else { return };
        let outcome = match result {
            _ if u.cancelled => Err(cancelled()),
            Ok(_) => Ok(false),
            Err(e) if e.code == ErrorCode::UploadMismatch => Err(ErrorObject { message: changed(&u.name).message, ..e }),
            Err(e) => Err(e),
        };
        self.upload_finish(handle, outcome);
    }

    pub(super) fn on_upload_cancelled(&mut self, handle: &str) {
        self.upload_finish(handle, Err(cancelled()));
    }

    /// The connection to `target` is gone: every upload on it that had begun fails.
    pub(super) fn uploads_on_closed(&mut self, target: &TargetId) {
        let mut failed: Vec<(u64, String)> = self
            .uploads
            .values()
            .filter(|u| &u.target == target && u.phase != UploadPhase::Hashing)
            .map(|u| (u.seq, u.handle.clone()))
            .collect();
        failed.sort_unstable();
        let name = self.targets.get(target).map_or_else(|| target.to_string(), |t| t.def.name.as_str().to_owned());
        for (_, handle) in failed {
            let message = format!("the connection to {name} dropped during the upload; attach the file again");
            self.upload_finish(&handle, Err(err(ErrorCode::TargetOffline, message)));
        }
    }

    // ── Moving bytes ─────────────────────────────────────────────────────────────────

    /// Starts queued uploads while the target has free slots.
    fn upload_admit(&mut self, target: &TargetId) {
        let max_active = self.cfg.uploads.max_active;
        loop {
            let on_target = || self.uploads.values().filter(|u| &u.target == target);
            let active = on_target().filter(|u| !matches!(u.phase, UploadPhase::Hashing | UploadPhase::Queued)).count();
            if active >= max_active {
                return;
            }
            let Some(next) = on_target().filter(|u| u.phase == UploadPhase::Queued).min_by_key(|u| u.seq).map(|u| u.handle.clone()) else {
                return;
            };
            let Some(u) = self.uploads.get(&next) else { return };
            let params = UploadBeginParams {
                name: u.name.clone(),
                size: u.size,
                media_type: u.media_type.clone(),
                sha256: u.sha256.clone().unwrap_or_else(|| Sha256Hex::new("")),
            };
            let generation = self.targets.get(target).and_then(|t| t.conn.as_ref()).filter(|c| c.ready).map(|c| c.generation);
            let Some(generation) = generation else {
                let name = self.targets.get(target).map_or_else(|| target.to_string(), |t| t.def.name.as_str().to_owned());
                self.upload_finish(&next, Err(err(ErrorCode::TargetOffline, format!("{name} is not connected; attach the file again when it is"))));
                continue;
            };
            if !self.send::<UploadBegin>(target, &params, Pending::UploadBegin { handle: next.clone() }) {
                self.upload_finish(&next, Err(err(ErrorCode::TargetOffline, "the upload could not be started on the target")));
                continue;
            }
            if let Some(u) = self.uploads.get_mut(&next) {
                u.phase = UploadPhase::Beginning;
                u.generation = generation;
            }
            self.upload_progress(&next, true);
            self.flush(target);
        }
    }

    /// Called before every write pass on `target`: starts pool reads while the scheduler is
    /// low on bytes, closes streams whose bytes are all queued, and sends `upload/end` once
    /// the last byte has been handed to the writer.
    pub(super) fn upload_pump(&mut self, target: &TargetId) {
        let Some(conn) = self.targets.get(target).and_then(|t| t.conn.as_ref()).filter(|c| c.ready) else { return };
        let generation = conn.generation;
        let sending: Vec<String> = self
            .uploads
            .values()
            .filter(|u| &u.target == target && u.phase == UploadPhase::Sending && !u.cancelled && u.generation == generation)
            .map(|u| u.handle.clone())
            .collect();
        for handle in sending {
            let Some(conn) = self.targets.get_mut(target).and_then(super::Target::ready_conn) else { return };
            let Some(u) = self.uploads.get_mut(&handle) else { continue };
            let Some(stream) = u.stream else { continue };
            if !u.closed && !u.reading {
                if u.read_offset >= u.size {
                    conn.session.close_stream(stream);
                    u.closed = true;
                } else if conn.session.scheduler().pending_data(stream).unwrap_or(0) < READ_LOW {
                    u.reading = true;
                    let (h, offset, len) = (handle.clone(), u.read_offset, READ_CHUNK.min(u.size - u.read_offset));
                    let Some(path) = u.path.clone() else { continue };
                    self.pool.submit(move || {
                        let result = read_chunk(&path, offset, len);
                        Some(CoreEvent::UploadChunk { handle: h, offset, result })
                    });
                }
            }
            let Some(u) = self.uploads.get_mut(&handle) else { continue };
            let drained = self.targets.get(target).and_then(|t| t.conn.as_ref()).is_some_and(|c| c.session.scheduler().pending_data(stream).is_none());
            if u.closed && drained {
                let Some(upload_id) = u.upload_id.clone() else { continue };
                u.phase = UploadPhase::Verifying;
                if !self.send::<UploadEnd>(target, &UploadEndParams { upload_id }, Pending::UploadEnd { handle: handle.clone() }) {
                    self.upload_finish(&handle, Err(err(ErrorCode::TargetOffline, "the upload could not be finished on the target")));
                    continue;
                }
                self.upload_progress(&handle, true);
            } else {
                self.upload_progress(&handle, false);
            }
        }
    }

    /// Fails an upload that has begun on the target, telling the target to drop it.
    fn upload_abort(&mut self, handle: &str, error: ErrorObject) {
        if let Some(u) = self.uploads.get(handle) {
            let (target, stream, upload_id) = (u.target.clone(), u.stream, u.upload_id.clone());
            if let Some(conn) = self.targets.get_mut(&target).and_then(super::Target::ready_conn)
                && let Some(stream) = stream
            {
                conn.session.cancel_stream(stream);
            }
            if let Some(upload_id) = upload_id {
                self.send::<methods::UploadCancel>(&target, &UploadCancelParams { upload_id }, Pending::Ignore);
                self.flush(&target);
            }
        }
        self.upload_finish(handle, Err(error));
    }

    /// Ends an upload: `Ok(skipped)` or the error. Sends its one `hub/upload/done`.
    fn upload_finish(&mut self, handle: &str, outcome: Result<bool, ErrorObject>) {
        let Some(u) = self.uploads.remove(handle) else { return };
        u.stop.store(true, Ordering::Relaxed);
        let (skipped, error) = match outcome {
            Ok(skipped) => (skipped, None),
            Err(e) => (false, Some(e)),
        };
        let done = UploadDoneParams {
            handle: u.handle.clone(),
            target: u.target.clone(),
            draft: u.draft.clone(),
            upload_id: if error.is_none() { u.upload_id.clone() } else { None },
            name: u.name.clone(),
            size: u.size,
            media_type: u.media_type.clone(),
            sha256: u.sha256.clone(),
            skipped,
            error,
        };
        self.notify_all(ext::names::UPLOAD_DONE, to_value(&done));
        self.upload_admit(&u.target);
    }

    fn progress_of(&self, u: &Upload) -> UploadProgressParams {
        let pending = u
            .stream
            .and_then(|s| self.targets.get(&u.target).and_then(|t| t.conn.as_ref()).and_then(|c| c.session.scheduler().pending_data(s)))
            .unwrap_or(0);
        let sent = match u.phase {
            UploadPhase::Sending => u.read_offset.saturating_sub(pending),
            UploadPhase::Verifying => u.size,
            _ => 0,
        };
        UploadProgressParams {
            handle: u.handle.clone(),
            target: u.target.clone(),
            draft: u.draft.clone(),
            upload_id: u.upload_id.clone(),
            phase: u.phase,
            sent,
            total: u.size,
        }
    }

    /// Broadcasts progress: always when `force`, else at most every [`PROGRESS_EVERY`].
    fn upload_progress(&mut self, handle: &str, force: bool) {
        let Some(u) = self.uploads.get(handle) else { return };
        let now = Instant::now();
        if !force && u.last_progress.is_some_and(|t| now.duration_since(t) < PROGRESS_EVERY) {
            return;
        }
        let params = to_value(&self.progress_of(u));
        if let Some(u) = self.uploads.get_mut(handle) {
            u.last_progress = Some(now);
        }
        self.notify_all(ext::names::UPLOAD_PROGRESS, params);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffs_images_by_content() {
        assert_eq!(sniff(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0]), Some("image/png"));
        assert_eq!(sniff(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(sniff(b"GIF89a"), Some("image/gif"));
        assert_eq!(sniff(b"plain text"), None);
        assert_eq!(sniff(&[0xff]), None);
    }

    #[test]
    fn hashes_a_file_and_stops_when_asked() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, b"abc").unwrap();
        let h = hash_file(&path, 10, &AtomicBool::new(false)).unwrap();
        assert_eq!(h.size, 3);
        assert_eq!(h.sha256, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let too_big = hash_file(&path, 2, &AtomicBool::new(false)).unwrap_err();
        assert_eq!(too_big.code, ErrorCode::LimitExceeded);
        assert!(too_big.message.contains("a.txt"), "{}", too_big.message);
        assert_eq!(hash_file(&path, 10, &AtomicBool::new(true)).unwrap_err().code.as_str(), ext::codes::UPLOAD_CANCELLED);
        assert_eq!(hash_file(dir.path(), 10, &AtomicBool::new(false)).unwrap_err().code.as_str(), ext::codes::FILE_UNREADABLE);
        assert_eq!(read_chunk(&path, 1, 10).unwrap(), b"bc");
    }
}
