//! Attachments through the client (docs/06-attachments.md): a chip follows the hub's
//! `hub/upload/progress` and `hub/upload/done`, routes by the model's image support, holds the
//! message back while unfinished, cancels when removed, re-uploads when its draft moves to
//! another target, and comes back after a window restart.
mod common;

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use common::{NOW, settle};
use serde_json::{Value, json};
use yaac_core::attachments::{ChipStatus, DraftRef, Phase, Upload, names};
use yaac_core::client::Client;
use yaac_core::demo::{DEMO_CLIENT, DemoHub, Scenario, threads};
use yaac_core::header::{ComposerView, composer};
use yaac_core::images::ImageRoute;
use yaac_core::link::HubLink;
use yaac_core::state::SendAs;
use yaac_protocol::rpc::{Message, RpcId};
use yaac_protocol::{ClientId, Support};

/// The demo hub for everything but uploads and blobs, which the test answers itself; every
/// message the client sends is recorded.
#[derive(Clone, Default)]
struct Wire {
    sent: Rc<RefCell<Vec<Message>>>,
    inject: Rc<RefCell<VecDeque<Message>>>,
}

struct Rig {
    hub: DemoHub,
    wire: Wire,
}

impl HubLink for Rig {
    fn send(&mut self, message: Message) {
        self.wire.sent.borrow_mut().push(message.clone());
        if let Message::Request { method, .. } = &message
            && (method.starts_with("hub/upload/") || method.starts_with("hub/drafts/blob/"))
        {
            return;
        }
        self.hub.send(message);
    }
    fn poll(&mut self) -> Option<Message> {
        let next = self.wire.inject.borrow_mut().pop_front();
        next.or_else(|| self.hub.poll())
    }
}

impl Wire {
    /// Requests of one method, oldest first.
    fn requests(&self, method: &str) -> Vec<(RpcId, Value)> {
        self.sent
            .borrow()
            .iter()
            .filter_map(|m| match m {
                Message::Request { id, method: mm, params } if mm == method => Some((id.clone(), params.clone())),
                _ => None,
            })
            .collect()
    }

    /// Routed requests (`hub/request`) of one method.
    fn routed(&self, method: &str) -> Vec<Value> {
        self.requests("hub/request").into_iter().filter(|(_, p)| p["method"] == method).map(|(_, p)| p["params"].clone()).collect()
    }

    fn last(&self, method: &str) -> (RpcId, Value) {
        self.requests(method).pop().unwrap_or_else(|| panic!("no {method} was sent"))
    }

    fn answer(&self, id: RpcId, result: Value) {
        self.inject.borrow_mut().push_back(Message::Response { id, result });
    }

    fn notify(&self, method: &str, params: Value) {
        self.inject.borrow_mut().push_back(Message::notification(method, params));
    }
}

fn boot_on(hub: &DemoHub) -> (Client, Wire) {
    let wire = Wire::default();
    let mut c = Client::new(Box::new(Rig { hub: hub.clone(), wire: wire.clone() }), "test", ClientId::new(DEMO_CLIENT));
    c.set_warm(hub.all_threads());
    settle(&mut c, hub);
    (c, wire)
}

fn boot() -> (Client, Wire, DemoHub) {
    let hub = DemoHub::new(Scenario::Fleet, NOW);
    let (c, wire) = boot_on(&hub);
    (c, wire, hub)
}

fn chip_status(c: &Client, d: &DraftRef, id: u64) -> ChipStatus {
    let a = c.draft(d).unwrap().attachments.iter().find(|a| a.id == id).unwrap();
    yaac_core::attachments::chip(a, &Support::Supported, c.draft_target(d).as_ref()).status
}

fn set_support(c: &mut Client, key: &yaac_core::ThreadKey, s: Support) {
    let t = c.state.transcripts.get_mut(key).expect("open");
    t.effective.as_mut().expect("effective capabilities").image_input = s;
}

fn view(c: &Client, key: &yaac_core::ThreadKey) -> (Vec<yaac_core::header::ChipView>, Option<String>) {
    let map = yaac_core::attention::AttentionMap::compute(&c.state);
    match composer(&c.state, &map, key) {
        ComposerView::Shown { chips, blocked, .. } => (chips, blocked),
        ComposerView::Absent { .. } => panic!("no composer"),
    }
}

#[test]
fn a_chip_follows_progress_and_done_and_its_image_reaches_the_model_as_an_image() {
    let (mut c, wire, hub) = boot();
    let fix = threads::billing();
    c.open(&fix);
    settle(&mut c, &hub);
    let d = DraftRef::Thread(fix.clone());
    let ids = c.attach_files(&d, &["/home/dev/shot.png".to_owned()]);
    let id = ids[0];
    // The upload starts when the attachment is added, not at send.
    let (rpc, start) = wire.last(names::UPLOAD_START);
    assert_eq!(start, json!({ "target": "devbox", "draft": "thread:devbox/t-billing", "name": "shot.png", "path": "/home/dev/shot.png" }));
    assert_eq!(chip_status(&c, &d, id), ChipStatus::Uploading { fraction: None, phase: Phase::Queued });
    c.state.drafts.get_mut(&fix).unwrap().text = "Look at this".into();
    // Sending is held back, and the button says why.
    assert_eq!(view(&c, &fix).1.as_deref(), Some("Waiting for 1 upload"));
    assert!(!c.send(&fix), "nothing is sent while an upload is unfinished");
    // A progress notification may overtake the answer: it is applied once the handle is known.
    wire.notify(names::UPLOAD_PROGRESS, json!({ "handle": "hu_1", "target": "devbox", "phase": "sending", "sent": 50, "total": 200 }));
    c.pump();
    wire.answer(rpc, json!({ "handle": "hu_1" }));
    c.pump();
    assert_eq!(chip_status(&c, &d, id), ChipStatus::Uploading { fraction: Some(0.25), phase: Phase::Sending });
    wire.notify(names::UPLOAD_PROGRESS, json!({ "handle": "hu_1", "target": "devbox", "phase": "sending", "sent": 150, "total": 200 }));
    c.pump();
    assert_eq!(chip_status(&c, &d, id), ChipStatus::Uploading { fraction: Some(0.75), phase: Phase::Sending });
    // Another window's upload is not ours.
    wire.notify(names::UPLOAD_DONE, json!({ "handle": "hu_other", "target": "devbox", "uploadId": "up_x", "name": "x", "size": 1 }));
    wire.notify(names::UPLOAD_DONE, json!({ "handle": "hu_1", "target": "devbox", "uploadId": "up_1", "name": "shot.png", "size": 200, "mediaType": "image/png", "skipped": false }));
    c.pump();
    assert_eq!(chip_status(&c, &d, id), ChipStatus::Ready);
    // The routing follows the model selected now.
    set_support(&mut c, &fix, Support::Unsupported);
    let (chips, blocked) = view(&c, &fix);
    assert_eq!((chips[0].image, chips[0].route, blocked), (true, ImageRoute::AsFileCannotSee, None));
    set_support(&mut c, &fix, Support::Unknown);
    let (chips, blocked) = view(&c, &fix);
    assert_eq!(chips[0].route, ImageRoute::Ask);
    assert_eq!(blocked.as_deref(), Some("Choose how to send shot.png"));
    assert!(!c.send(&fix));
    c.choose_send_as(&d, id, SendAs::Image);
    set_support(&mut c, &fix, Support::Supported);
    assert_eq!(view(&c, &fix).1, None);
    assert!(c.send(&fix));
    let sent = wire.routed("turn/start").pop().unwrap();
    assert_eq!(sent["input"][1], json!({ "type": "image", "uploadId": "up_1", "name": "shot.png", "mediaType": "image/png" }));
    settle(&mut c, &hub);
    assert!(c.draft(&d).is_none_or(|x| x.attachments.is_empty()), "the composer is empty after sending");
}

#[test]
fn an_unsupported_image_is_sent_as_a_file_and_a_failure_shows_the_hubs_text() {
    let (mut c, wire, hub) = boot();
    let fix = threads::billing();
    c.open(&fix);
    settle(&mut c, &hub);
    set_support(&mut c, &fix, Support::Unsupported);
    let d = DraftRef::Thread(fix.clone());
    let ids = c.attach_files(&d, &["/tmp/diagram.png".to_owned(), "/tmp/huge.bin".to_owned()]);
    let starts = wire.requests(names::UPLOAD_START);
    assert_eq!(starts.len(), 2);
    wire.answer(starts[0].0.clone(), json!({ "handle": "hu_a" }));
    wire.answer(starts[1].0.clone(), json!({ "handle": "hu_b" }));
    wire.notify(names::UPLOAD_DONE, json!({ "handle": "hu_a", "target": "devbox", "uploadId": "up_a", "name": "diagram.png", "size": 9, "mediaType": "image/png" }));
    wire.notify(names::UPLOAD_DONE, json!({ "handle": "hu_b", "target": "devbox", "name": "huge.bin", "size": 0,
        "error": { "code": "limit_exceeded", "message": "huge.bin is larger than 512 MiB." } }));
    c.pump();
    assert_eq!(chip_status(&c, &d, ids[1]), ChipStatus::Failed { message: "huge.bin is larger than 512 MiB.".into(), retry: true });
    assert_eq!(view(&c, &fix).1.as_deref(), Some("huge.bin was not uploaded · remove it or retry"));
    // Retry asks again; removing it lets the message go.
    c.retry_attachment(&d, ids[1]);
    assert_eq!(wire.requests(names::UPLOAD_START).len(), 3);
    c.remove_attachment(&d, ids[1]);
    c.state.drafts.get_mut(&fix).unwrap().text = "see".into();
    assert!(c.send(&fix));
    let sent = wire.routed("turn/start").pop().unwrap();
    assert_eq!(sent["input"][1]["type"], "file", "an image for a model that cannot see goes as a file");
    assert_eq!(sent["input"][1]["uploadId"], "up_a");
    // The retry's answer arrives after the chip was removed: that upload is cancelled.
    let retry = wire.requests(names::UPLOAD_START)[2].0.clone();
    wire.answer(retry, json!({ "handle": "hu_c" }));
    c.pump();
    assert_eq!(wire.last(names::UPLOAD_CANCEL).1, json!({ "handle": "hu_c" }));
}

#[test]
fn removing_a_chip_cancels_its_upload() {
    let (mut c, wire, hub) = boot();
    let fix = threads::fix();
    c.open(&fix);
    settle(&mut c, &hub);
    let d = DraftRef::Thread(fix);
    let id = c.attach_files(&d, &["/tmp/trace.log".to_owned()])[0];
    wire.answer(wire.last(names::UPLOAD_START).0, json!({ "handle": "hu_9" }));
    c.pump();
    assert!(matches!(c.draft(&d).unwrap().attachments[0].upload, Upload::Running { .. }));
    c.remove_attachment(&d, id);
    assert_eq!(wire.last(names::UPLOAD_CANCEL).1, json!({ "handle": "hu_9" }));
    assert!(c.draft(&d).unwrap().attachments.is_empty());
    // Its cancellation report changes nothing.
    wire.notify(names::UPLOAD_DONE, json!({ "handle": "hu_9", "target": "devbox", "name": "trace.log", "size": 0,
        "error": { "code": "upload_cancelled", "message": "the upload was cancelled" } }));
    c.pump();
    assert!(c.draft(&d).unwrap().attachments.is_empty());
}

#[test]
fn clipboard_bytes_are_stored_as_a_blob_then_uploaded_from_it() {
    let (mut c, wire, hub) = boot();
    let fix = threads::fix();
    c.open(&fix);
    settle(&mut c, &hub);
    let d = DraftRef::Thread(fix);
    let chunks = vec!["AAAA".to_owned(), "AA==".to_owned()];
    let id = c.attach_bytes(&d, "screenshot-2026-11-20-1405.png", Some("image/png"), 4, chunks).unwrap();
    assert_eq!(chip_status(&c, &d, id), ChipStatus::Storing);
    let (begin, p) = wire.last(names::BLOB_BEGIN);
    assert_eq!(p, json!({ "draft": "thread:devbox/t-fix", "name": "screenshot-2026-11-20-1405.png", "size": 4, "mediaType": "image/png" }));
    wire.answer(begin, json!({ "blobId": "b_1" }));
    c.pump();
    let appends = wire.requests(names::BLOB_APPEND);
    assert_eq!(appends.iter().map(|(_, p)| p["offset"].as_u64().unwrap()).collect::<Vec<_>>(), [0, 3], "offsets follow the decoded bytes");
    let (end, p) = wire.last(names::BLOB_END);
    assert_eq!(p, json!({ "blobId": "b_1" }));
    assert!(wire.requests(names::UPLOAD_START).is_empty(), "not uploaded before the blob is complete");
    wire.answer(end, json!({ "blobId": "b_1", "draft": "thread:devbox/t-fix", "name": "screenshot-2026-11-20-1405.png", "mediaType": "image/png", "size": 4, "sha256": "00" }));
    c.pump();
    let (_, start) = wire.last(names::UPLOAD_START);
    assert_eq!(start["blobId"], "b_1");
    assert_eq!(start["mediaType"], "image/png");
    // Removing it forgets the stored bytes too.
    c.remove_attachment(&d, id);
    assert_eq!(wire.last(names::BLOB_DELETE).1, json!({ "blobId": "b_1" }));
}

#[test]
fn moving_a_draft_to_another_target_uploads_its_attachments_again() {
    let (mut c, wire, hub) = boot();
    let local_project = c.state.projects.iter().find(|p| p.target.as_str() == "local").expect("a project on this computer").id.clone();
    let devbox_project = c.state.projects.iter().find(|p| p.target.as_str() == "devbox").expect("a project on devbox").id.clone();
    let id = c.new_thread("local".into(), None);
    c.set_draft_project(id, &local_project);
    let d = DraftRef::New(id);
    let a = c.attach_files(&d, &["/home/dev/notes.md".to_owned()])[0];
    let (rpc, p) = wire.last(names::UPLOAD_START);
    assert_eq!((p["target"].as_str(), p["draft"].as_str()), (Some("local"), Some("new:1")));
    wire.answer(rpc, json!({ "handle": "hu_l" }));
    wire.notify(names::UPLOAD_DONE, json!({ "handle": "hu_l", "target": "local", "uploadId": "up_l", "name": "notes.md", "size": 3 }));
    c.pump();
    assert_eq!(chip_status(&c, &d, a), ChipStatus::Ready);
    c.set_draft_project(id, &devbox_project);
    // The upload id of this computer means nothing on devbox: the chip uploads again.
    assert!(chip_status(&c, &d, a).pending());
    let (rpc, p) = wire.last(names::UPLOAD_START);
    assert_eq!((p["target"].as_str(), p["path"].as_str()), (Some("devbox"), Some("/home/dev/notes.md")));
    assert!(!c.start_thread(id), "not sent before the new upload is done");
    wire.answer(rpc, json!({ "handle": "hu_d" }));
    c.pump();
    // Moved again while uploading: the running upload is cancelled first.
    c.set_draft_project(id, &local_project);
    assert_eq!(wire.last(names::UPLOAD_CANCEL).1, json!({ "handle": "hu_d" }));
    assert_eq!(wire.last(names::UPLOAD_START).1["target"], "local");
    settle(&mut c, &hub);
}

#[test]
fn drafts_keep_their_chips_across_a_window_restart() {
    let hub = DemoHub::new(Scenario::Fleet, NOW);
    let (mut a, wire_a) = boot_on(&hub);
    let fix = threads::fix();
    a.open(&fix);
    settle(&mut a, &hub);
    let d = DraftRef::Thread(fix.clone());
    let ids = a.attach_files(&d, &["/tmp/done.txt".to_owned(), "/tmp/running.txt".to_owned(), "/tmp/queued.txt".to_owned()]);
    let starts = wire_a.requests(names::UPLOAD_START);
    wire_a.answer(starts[0].0.clone(), json!({ "handle": "hu_1" }));
    wire_a.answer(starts[1].0.clone(), json!({ "handle": "hu_2" }));
    wire_a.notify(names::UPLOAD_DONE, json!({ "handle": "hu_1", "target": "devbox", "uploadId": "up_1", "name": "done.txt", "size": 1 }));
    a.pump();
    a.state.drafts.get_mut(&fix).unwrap().text = "half-written".into();
    a.draft_changed(&d);
    a.flush_drafts();
    settle(&mut a, &hub);
    let put = wire_a.last(names::DRAFTS_PUT).1;
    assert_eq!(put["key"], "thread:devbox/t-fix");
    assert_eq!(put["attachments"].as_array().unwrap().len(), 3);
    assert!(!a.drafts_dirty());
    drop(a);

    // A new window on the same hub.
    let (mut b, wire_b) = boot_on(&hub);
    let draft = b.state.drafts.get(&fix).expect("the draft came back").clone();
    assert_eq!(draft.text, "half-written");
    let names_: Vec<&str> = draft.attachments.iter().map(|x| x.name.as_str()).collect();
    assert_eq!(names_, ["done.txt", "running.txt", "queued.txt"]);
    assert!(matches!(draft.attachments[0].upload, Upload::Done { .. }), "a finished upload is kept");
    // The one that never got a handle is started again at once; the running one is checked
    // against the hub's list, and started again because the hub no longer has it.
    let starts = wire_b.requests(names::UPLOAD_START);
    let paths: Vec<&str> = starts.iter().map(|(_, p)| p["path"].as_str().unwrap()).collect();
    assert!(paths.contains(&"/tmp/queued.txt"), "{paths:?}");
    let (list, _) = wire_b.last(names::UPLOAD_LIST);
    wire_b.answer(list, json!({ "uploads": [] }));
    b.pump();
    let paths: Vec<String> = wire_b.requests(names::UPLOAD_START).iter().map(|(_, p)| p["path"].as_str().unwrap().to_owned()).collect();
    assert!(paths.iter().any(|p| p == "/tmp/running.txt"), "{paths:?}");
    let _ = ids;
}

#[test]
fn the_demo_hub_plays_an_upload_to_the_end() {
    let hub = DemoHub::new(Scenario::Fleet, NOW);
    let mut c = Client::new(Box::new(hub.clone()), "test", ClientId::new(DEMO_CLIENT));
    settle(&mut c, &hub);
    let fix = threads::fix();
    c.open(&fix);
    settle(&mut c, &hub);
    let d = DraftRef::Thread(fix.clone());
    let id = c.attach_files(&d, &["/home/dev/plot.png".to_owned()])[0];
    let mut seen_fraction = false;
    for _ in 0..200 {
        c.pump();
        if matches!(chip_status(&c, &d, id), ChipStatus::Uploading { fraction: Some(f), .. } if f > 0.0 && f < 1.0) {
            seen_fraction = true;
        }
        if !hub.tick() {
            break;
        }
    }
    c.pump();
    assert!(seen_fraction);
    assert_eq!(chip_status(&c, &d, id), ChipStatus::Ready);
    assert!(c.draft(&d).unwrap().attachments[0].is_image());
}
