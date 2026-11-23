//! The client's side of attachments (docs/06-attachments.md): adding, uploading, removing,
//! retargeting, and keeping drafts in the hub's durable draft store so a window restart keeps
//! them. See [`crate::attachments`] for the chip's states.

use serde_json::{Value, json};
use yaac_protocol::rpc::ErrorObject;
use yaac_protocol::{TargetId, ThreadId, UploadId};

use super::{Call, Client};
use crate::attachments::{DraftAttachment, DraftRef, Phase, Source, Upload, names};
use crate::key::ThreadKey;
use crate::state::{Draft, NewThreadDraft, SendAs};

/// How many upload notifications for not-yet-known handles are kept.
const ORPHANS: usize = 64;

/// The one element of a new-thread draft's stored `attachments` that is not a chip: what the
/// draft was set to (its project and selections), so it comes back as it was.
const NEW_THREAD_META: &str = "newThread";

impl Client {
    /// A composer's content.
    pub fn draft(&self, d: &DraftRef) -> Option<&Draft> {
        match d {
            DraftRef::Thread(k) => self.state.drafts.get(k),
            DraftRef::New(id) => self.state.new_threads.iter().find(|x| x.id == *id).map(|x| &x.draft),
        }
    }

    fn draft_mut(&mut self, d: &DraftRef) -> Option<&mut Draft> {
        match d {
            DraftRef::Thread(k) => Some(self.state.drafts.entry(k.clone()).or_default()),
            DraftRef::New(id) => self.state.new_threads.iter_mut().find(|x| x.id == *id).map(|x| &mut x.draft),
        }
    }

    /// The target a composer's message goes to.
    pub fn draft_target(&self, d: &DraftRef) -> Option<TargetId> {
        match d {
            DraftRef::Thread(k) => Some(k.target.clone()),
            DraftRef::New(id) => self.state.new_threads.iter().find(|x| x.id == *id).map(|x| x.target.clone()),
        }
    }

    fn attachment_mut(&mut self, d: &DraftRef, id: u64) -> Option<&mut DraftAttachment> {
        self.draft_mut(d)?.attachments.iter_mut().find(|a| a.id == id)
    }

    fn next_attachment(&mut self) -> u64 {
        self.attach_ids += 1;
        self.attach_ids
    }

    /// The composer's text changed (or a selection of a new-thread draft): store it soon.
    pub fn draft_changed(&mut self, d: &DraftRef) {
        self.dirty.insert(d.clone());
    }

    /// Attach files by path (the file picker, a drop, a pasted file list). Each gets a chip at
    /// once and its upload starts now, not at send. Returns the new chips' ids.
    pub fn attach_files(&mut self, d: &DraftRef, paths: &[String]) -> Vec<u64> {
        let mut ids = Vec::new();
        for path in paths {
            let name = path.rsplit(['/', '\\']).next().filter(|n| !n.is_empty()).unwrap_or(path).to_owned();
            let id = self.next_attachment();
            let a = DraftAttachment::new(id, name, Some(Source::Path { path: path.clone() }), None);
            let Some(draft) = self.draft_mut(d) else { return ids };
            draft.attachments.push(a);
            self.start_upload(d, id);
            ids.push(id);
        }
        self.dirty.insert(d.clone());
        ids
    }

    /// Attach bytes that exist only in the window (a clipboard image, a long pasted text):
    /// they are stored in the hub's draft store first (`chunks` are the base64 pieces of at
    /// most [`crate::attachments::BLOB_CHUNK_MAX`] bytes each, made off the UI thread), then
    /// uploaded from there.
    pub fn attach_bytes(&mut self, d: &DraftRef, name: &str, media_type: Option<&str>, size: u64, chunks: Vec<String>) -> Option<u64> {
        let id = self.next_attachment();
        let mut a = DraftAttachment::new(id, name, None, media_type.map(str::to_owned));
        a.size = Some(size);
        a.upload = Upload::Storing;
        self.draft_mut(d)?.attachments.push(a);
        let mut params = json!({ "draft": d.store_key(), "name": name, "size": size });
        if let Some(m) = media_type {
            params["mediaType"] = json!(m);
        }
        self.request(names::BLOB_BEGIN, params, Call::BlobBegin { draft: d.clone(), id, chunks });
        self.dirty.insert(d.clone());
        Some(id)
    }

    /// The × on a chip: a running upload is cancelled, stored bytes are forgotten.
    pub fn remove_attachment(&mut self, d: &DraftRef, id: u64) {
        let Some(draft) = self.draft_mut(d) else { return };
        let Some(i) = draft.attachments.iter().position(|a| a.id == id) else { return };
        let a = draft.attachments.remove(i);
        if let Upload::Running { handle, .. } = &a.upload {
            self.request(names::UPLOAD_CANCEL, json!({ "handle": handle }), Call::Ignore);
        }
        if let Some(Source::Blob { blob_id }) = &a.source {
            self.request(names::BLOB_DELETE, json!({ "blobId": blob_id }), Call::Ignore);
        }
        self.dirty.insert(d.clone());
    }

    /// "Send as image" / "as file" on a chip whose model's image support is unknown.
    pub fn choose_send_as(&mut self, d: &DraftRef, id: u64, choice: SendAs) {
        if let Some(a) = self.attachment_mut(d, id) {
            a.choice = Some(choice);
            self.dirty.insert(d.clone());
        }
    }

    /// "Retry" on a failed chip whose bytes are still there.
    pub fn retry_attachment(&mut self, d: &DraftRef, id: u64) {
        if self.attachment_mut(d, id).is_some_and(|a| a.source.is_some()) {
            self.start_upload(d, id);
        }
    }

    /// Upload a chip from its source to its draft's target (a new attempt: answers to an older
    /// one are ignored, and an older handle is cancelled).
    fn start_upload(&mut self, d: &DraftRef, id: u64) {
        let Some(target) = self.draft_target(d) else { return };
        let Some(a) = self.attachment_mut(d, id) else { return };
        let Some(source) = a.source.clone() else {
            a.upload = Upload::Failed { message: "This attachment is gone; attach it again.".into(), offline: false };
            return;
        };
        let old = match &a.upload {
            Upload::Running { handle, .. } => Some(handle.clone()),
            _ => None,
        };
        a.attempt += 1;
        a.upload = Upload::Starting;
        let attempt = a.attempt;
        let mut params = json!({ "target": target, "draft": d.store_key(), "name": a.name });
        match source {
            Source::Path { path } => params["path"] = json!(path),
            Source::Blob { blob_id } => params["blobId"] = json!(blob_id),
        }
        if let Some(m) = &a.media_type {
            params["mediaType"] = json!(m);
        }
        if let Some(h) = old {
            self.request(names::UPLOAD_CANCEL, json!({ "handle": h }), Call::Ignore);
        }
        self.request(names::UPLOAD_START, params, Call::UploadStart { draft: d.clone(), id, attempt });
        self.dirty.insert(d.clone());
    }

    /// The draft's target changed: its uploads are invalid there and are sent again.
    pub(super) fn retarget_uploads(&mut self, d: &DraftRef) {
        let ids: Vec<u64> = self.draft(d).map(|x| x.attachments.iter().map(|a| a.id).collect()).unwrap_or_default();
        for id in ids {
            if self.attachment_mut(d, id).is_some_and(|a| !matches!(a.upload, Upload::Storing)) {
                self.start_upload(d, id);
            }
        }
    }

    /// A target is back: uploads that failed because it dropped are asked again.
    pub(super) fn resume_offline_uploads(&mut self, target: &TargetId) {
        let mut todo = Vec::new();
        for (k, draft) in &self.state.drafts {
            if &k.target == target {
                todo.extend(draft.attachments.iter().filter(|a| matches!(a.upload, Upload::Failed { offline: true, .. })).map(|a| (DraftRef::Thread(k.clone()), a.id)));
            }
        }
        for d in self.state.new_threads.iter().filter(|d| &d.target == target) {
            todo.extend(d.draft.attachments.iter().filter(|a| matches!(a.upload, Upload::Failed { offline: true, .. })).map(|a| (DraftRef::New(d.id), a.id)));
        }
        for (d, id) in todo {
            self.start_upload(&d, id);
        }
    }

    pub(super) fn on_upload_started(&mut self, d: &DraftRef, id: u64, attempt: u32, result: &Value) {
        let Some(handle) = result.get("handle").and_then(Value::as_str).map(str::to_owned) else { return };
        match self.attachment_mut(d, id) {
            Some(a) if a.attempt == attempt => {
                a.upload = Upload::Running { handle: handle.clone(), phase: Phase::Queued, sent: 0, total: a.size.unwrap_or(0) };
            }
            // The chip was removed or started again meanwhile: this upload is not wanted.
            _ => {
                self.request(names::UPLOAD_CANCEL, json!({ "handle": handle }), Call::Ignore);
                return;
            }
        }
        self.dirty.insert(d.clone());
        // Notifications that overtook the answer.
        let early: Vec<Value> = self.orphans.iter().filter(|(h, _)| h == &handle).map(|(_, v)| v.clone()).collect();
        self.orphans.retain(|(h, _)| h != &handle);
        for n in early {
            let method = n.get("method").and_then(Value::as_str).unwrap_or_default().to_owned();
            self.on_upload_note(&method, n.get("params").unwrap_or(&Value::Null));
        }
    }

    pub(super) fn upload_failed(&mut self, d: &DraftRef, id: u64, attempt: u32, error: &ErrorObject) {
        if let Some(a) = self.attachment_mut(d, id)
            && (attempt == u32::MAX || a.attempt == attempt)
        {
            a.upload = Upload::Failed { message: error.message.clone(), offline: error.code.as_str() == names::TARGET_OFFLINE };
            self.dirty.insert(d.clone());
        }
    }

    pub(super) fn on_blob_begun(&mut self, d: &DraftRef, id: u64, chunks: Vec<String>, result: &Value) {
        let Some(blob_id) = result.get("blobId").and_then(Value::as_str).map(str::to_owned) else { return };
        if self.attachment_mut(d, id).is_none() {
            // Removed while it was being stored.
            self.request(names::BLOB_DELETE, json!({ "blobId": blob_id }), Call::Ignore);
            return;
        }
        let mut offset = 0u64;
        for chunk in chunks {
            let len = decoded_len(&chunk);
            self.request(names::BLOB_APPEND, json!({ "blobId": blob_id, "offset": offset, "dataBase64": chunk }), Call::BlobAppend { draft: d.clone(), id });
            offset += len;
        }
        self.request(names::BLOB_END, json!({ "blobId": blob_id }), Call::BlobEnd { draft: d.clone(), id });
    }

    pub(super) fn on_blob_ended(&mut self, d: &DraftRef, id: u64, result: &Value) {
        let Some(blob_id) = result.get("blobId").and_then(Value::as_str).map(str::to_owned) else { return };
        let Some(a) = self.attachment_mut(d, id) else {
            self.request(names::BLOB_DELETE, json!({ "blobId": blob_id }), Call::Ignore);
            return;
        };
        if matches!(a.upload, Upload::Failed { .. }) {
            // A chunk was refused; the chip says why.
            return;
        }
        a.source = Some(Source::Blob { blob_id });
        if let Some(size) = result.get("size").and_then(Value::as_u64) {
            a.size = Some(size);
        }
        self.start_upload(d, id);
    }

    /// `hub/upload/progress` and `hub/upload/done`, sent to every window: matched by handle.
    pub(super) fn on_upload_note(&mut self, method: &str, params: &Value) {
        let Some(handle) = params.get("handle").and_then(Value::as_str).map(str::to_owned) else { return };
        let Some((d, id)) = self.find_handle(&handle) else {
            // Another window's upload, or an answer still on its way: keep it a while.
            if self.orphans.len() >= ORPHANS {
                self.orphans.pop_front();
            }
            self.orphans.push_back((handle, json!({ "method": method, "params": params.clone() })));
            return;
        };
        let Some(a) = self.attachment_mut(&d, id) else { return };
        if method == names::UPLOAD_PROGRESS {
            let phase = Phase::parse(params.get("phase").and_then(Value::as_str).unwrap_or_default());
            let sent = params.get("sent").and_then(Value::as_u64).unwrap_or(0);
            let total = params.get("total").and_then(Value::as_u64).unwrap_or(0);
            a.upload = Upload::Running { handle, phase, sent, total };
            // Progress is not worth a store write.
            return;
        }
        if let Some(err) = params.get("error").and_then(|e| serde_json::from_value::<ErrorObject>(e.clone()).ok()) {
            a.upload = Upload::Failed { message: err.message.clone(), offline: err.code.as_str() == names::TARGET_OFFLINE };
        } else if let (Some(upload_id), Some(target)) = (params.get("uploadId").and_then(Value::as_str), params.get("target").and_then(Value::as_str)) {
            a.upload = Upload::Done { target: TargetId::new(target), upload_id: UploadId::from(upload_id) };
            if let Some(m) = params.get("mediaType").and_then(Value::as_str) {
                a.media_type = Some(m.to_owned());
            }
            if let Some(s) = params.get("size").and_then(Value::as_u64) {
                a.size = Some(s);
            }
            if let Some(n) = params.get("name").and_then(Value::as_str).filter(|n| !n.is_empty()) {
                a.name = n.to_owned();
            }
        }
        self.dirty.insert(d);
    }

    fn find_handle(&self, handle: &str) -> Option<(DraftRef, u64)> {
        let is = |a: &DraftAttachment| matches!(&a.upload, Upload::Running { handle: h, .. } if h == handle);
        for (k, draft) in &self.state.drafts {
            if let Some(a) = draft.attachments.iter().find(|a| is(a)) {
                return Some((DraftRef::Thread(k.clone()), a.id));
            }
        }
        self.state.new_threads.iter().find_map(|d| d.draft.attachments.iter().find(|a| is(a)).map(|a| (DraftRef::New(d.id), a.id)))
    }

    /// A message was sent (or may have been): its stored bytes are no longer needed, and the
    /// stored draft goes unless something new was written in the composer meanwhile.
    pub(super) fn sent(&mut self, d: &DraftRef, sent: &Draft) {
        for a in &sent.attachments {
            if let Some(Source::Blob { blob_id }) = &a.source {
                self.request(names::BLOB_DELETE, json!({ "blobId": blob_id }), Call::Ignore);
            }
        }
        if self.draft(d).is_none_or(Draft::is_empty) {
            self.dirty.remove(d);
            self.request(names::DRAFTS_DELETE, json!({ "key": d.store_key() }), Call::Ignore);
        } else {
            self.dirty.insert(d.clone());
        }
    }

    /// Whether drafts are waiting to be stored.
    pub fn drafts_dirty(&self) -> bool {
        !self.dirty.is_empty()
    }

    /// Store changed drafts in the hub (`hub/drafts/put`, or `hub/drafts/delete` for one that
    /// is now empty). The window calls this at a calm pace, not on every key.
    pub fn flush_drafts(&mut self) {
        if !self.state.hub.attached {
            return;
        }
        for d in std::mem::take(&mut self.dirty) {
            let key = d.store_key();
            let value = match &d {
                DraftRef::Thread(k) => self.state.drafts.get(k).filter(|x| !x.is_empty()).map(|x| {
                    json!({ "key": key, "target": k.target, "threadId": k.thread, "text": x.text, "attachments": stored(&x.attachments) })
                }),
                DraftRef::New(id) => self.state.new_threads.iter().find(|x| x.id == *id).map(|x| {
                    let mut atts = vec![new_thread_meta(x)];
                    atts.extend(stored(&x.draft.attachments));
                    json!({ "key": key, "target": x.target, "text": x.draft.text, "attachments": atts })
                }),
            };
            match value {
                Some(v) => self.request(names::DRAFTS_PUT, v, Call::Ignore),
                None => self.request(names::DRAFTS_DELETE, json!({ "key": key }), Call::Ignore),
            };
        }
    }

    /// The drafts the hub kept (`hub/drafts/list`, after attaching): put back the ones this
    /// window does not have, start the uploads that were not finished, and ask which of the
    /// running ones the hub still has.
    pub(super) fn on_drafts_list(&mut self, result: &Value) {
        let drafts = result.get("drafts").and_then(Value::as_array).cloned().unwrap_or_default();
        let mut restart = Vec::new();
        let mut running = false;
        for v in drafts {
            let key = v.get("key").and_then(Value::as_str).unwrap_or_default().to_owned();
            let text = v.get("text").and_then(Value::as_str).unwrap_or_default().to_owned();
            let target = v.get("target").and_then(Value::as_str).map(TargetId::new);
            let stored_atts = v.get("attachments").and_then(Value::as_array).cloned().unwrap_or_default();
            let meta = stored_atts.iter().find(|a| a.get("kind").and_then(Value::as_str) == Some(NEW_THREAD_META)).cloned();
            let mut attachments = Vec::new();
            for a in stored_atts.iter().filter(|a| a.get("kind").and_then(Value::as_str) != Some(NEW_THREAD_META)) {
                let id = self.next_attachment();
                if let Some(att) = DraftAttachment::from_stored(a, id) {
                    attachments.push(att);
                }
            }
            let draft = Draft { text, attachments };
            let d = if let Some(new_id) = DraftRef::new_id_of(&key) {
                let (Some(target), Some(meta)) = (target, meta) else { continue };
                if self.state.new_threads.iter().any(|x| x.id == new_id) {
                    continue;
                }
                let s = |k: &str| meta.get(k).and_then(Value::as_str).map(str::to_owned);
                self.state.new_threads.push(NewThreadDraft {
                    id: new_id,
                    target,
                    project: s("project"),
                    cwd: s("cwd").map(Into::into),
                    instance: s("instance").map(Into::into),
                    model: s("model").map(Into::into),
                    effort: s("effort").map(Into::into),
                    mode: s("mode").map(Into::into),
                    fast: meta.get("fast").and_then(Value::as_bool).unwrap_or(false),
                    ultrafast: meta.get("ultrafast").and_then(Value::as_bool).unwrap_or(false),
                    fast_note: None,
                    draft,
                });
                DraftRef::New(new_id)
            } else {
                let (Some(target), Some(thread)) = (target, v.get("threadId").and_then(Value::as_str).map(ThreadId::new)) else { continue };
                let k = ThreadKey::new(target, thread);
                if self.state.drafts.get(&k).is_some_and(|x| !x.is_empty()) {
                    continue;
                }
                self.state.drafts.insert(k.clone(), draft);
                DraftRef::Thread(k)
            };
            for a in self.draft(&d).map(|x| x.attachments.clone()).unwrap_or_default() {
                match a.upload {
                    Upload::Starting => restart.push((d.clone(), a.id)),
                    Upload::Running { .. } => running = true,
                    _ => {}
                }
            }
        }
        for (d, id) in restart {
            self.start_upload(&d, id);
        }
        if running {
            self.request(names::UPLOAD_LIST, json!({}), Call::UploadList);
        }
    }

    /// The hub's unfinished uploads: a restored chip whose upload the hub no longer has
    /// (it finished or failed while no window listened) is uploaded again — content addressing
    /// makes that cheap.
    pub(super) fn on_upload_list(&mut self, result: &Value) {
        let live: Vec<String> = result
            .get("uploads")
            .and_then(Value::as_array)
            .map(|u| u.iter().filter_map(|x| x.get("handle").and_then(Value::as_str).map(str::to_owned)).collect())
            .unwrap_or_default();
        let mut todo = Vec::new();
        let stale = |a: &DraftAttachment| matches!(&a.upload, Upload::Running { handle, .. } if !live.contains(handle));
        for (k, draft) in &self.state.drafts {
            todo.extend(draft.attachments.iter().filter(|a| stale(a)).map(|a| (DraftRef::Thread(k.clone()), a.id)));
        }
        for d in &self.state.new_threads {
            todo.extend(d.draft.attachments.iter().filter(|a| stale(a)).map(|a| (DraftRef::New(d.id), a.id)));
        }
        for (d, id) in todo {
            if let Some(a) = self.attachment_mut(&d, id) {
                // Not cancelled: the hub has no such handle any more.
                a.upload = Upload::Starting;
            }
            self.start_upload(&d, id);
        }
    }
}

fn stored(attachments: &[DraftAttachment]) -> Vec<Value> {
    attachments.iter().map(DraftAttachment::to_stored).collect()
}

fn new_thread_meta(d: &NewThreadDraft) -> Value {
    let mut v = json!({ "kind": NEW_THREAD_META });
    let mut put = |k: &str, x: Option<String>| {
        if let Some(x) = x {
            v[k] = json!(x);
        }
    };
    put("project", d.project.clone());
    put("cwd", d.cwd.as_ref().map(ToString::to_string));
    put("instance", d.instance.as_ref().map(ToString::to_string));
    put("model", d.model.as_ref().map(ToString::to_string));
    put("effort", d.effort.as_ref().map(ToString::to_string));
    put("mode", d.mode.as_ref().map(ToString::to_string));
    if d.fast {
        v["fast"] = json!(true);
    }
    if d.ultrafast {
        v["ultrafast"] = json!(true);
    }
    v
}

/// Bytes a padded standard base64 string decodes to.
fn decoded_len(b64: &str) -> u64 {
    let pad = b64.bytes().rev().take_while(|b| *b == b'=').count();
    (b64.len() / 4 * 3).saturating_sub(pad) as u64
}
