//! Uploads from the window through the hub to the **real** `yaacd` on its fake provider
//! (docs/06-attachments.md): the hub reads and hashes the file, sends it on the target's one
//! connection as bulk data under credit, and the target stores it by digest. Runs only when
//! `daemon/dist/yaacd.mjs` exists and `node` (or `YAAC_RUNTIME`) runs; otherwise it prints
//! that it was skipped and passes.
//!
//! Each daemon gets its own `YAAC_HOME`, socket, and `TMPDIR` (so its upload folder is inside
//! the test's temporary directory), and exits by itself when idle.

mod support;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use support::{BUILD, Client, Home, wait_until};
use yaac_hub::protocol::{BlobAppendParams, names as ext};
use yaac_hub::settings::TargetDef;
use yaac_hub::{ConnectorFactory, HubConfig};
use yaac_protocol::attach::ConnectionStatus;
use yaac_protocol::rpc::Message;
use yaac_protocol::{InputPart, Item, ItemBody, TargetId};
use yaac_transport::{Connector, LocalConfig, LocalConnector};

fn runnable() -> Option<(std::ffi::OsString, PathBuf)> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let bundle = LocalConfig::default_bundle(&root);
    if !bundle.is_file() {
        eprintln!("SKIPPED: daemon/dist/yaacd.mjs does not exist (build the daemon to run this test)");
        return None;
    }
    let runtime = std::env::var_os("YAAC_RUNTIME").unwrap_or_else(|| "node".into());
    if std::process::Command::new(&runtime).arg("--version").output().is_err() {
        eprintln!("SKIPPED: the runtime {runtime:?} does not run");
        return None;
    }
    Some((runtime, bundle))
}

/// One real daemon per target name.
struct Daemons(HashMap<String, LocalConfig>);

impl ConnectorFactory for Daemons {
    fn connector(&self, target: &TargetDef) -> Result<Arc<dyn Connector>, String> {
        let config = self.0.get(target.name.as_str()).ok_or_else(|| format!("no daemon named {}", target.name))?;
        Ok(Arc::new(LocalConnector::new(config.clone())))
    }
}

struct Rig {
    home: Home,
    /// Each daemon's home directory (also its working directory for threads).
    homes: HashMap<String, PathBuf>,
    /// Each daemon's temp directory (its upload folder is under it).
    temps: HashMap<String, PathBuf>,
    factory: Arc<Daemons>,
}

impl Rig {
    fn new(names: &[&str]) -> Option<Self> {
        let (runtime, bundle) = runnable()?;
        let home = Home::new();
        let (mut homes, mut temps, mut configs) = (HashMap::new(), HashMap::new(), HashMap::new());
        for name in names {
            let dir = home.dir.path().join(format!("target-{name}"));
            let tmp = home.dir.path().join(format!("tmp-{name}"));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::create_dir_all(&tmp).unwrap();
            let local = LocalConfig { runtime: runtime.clone(), bundle: bundle.clone(), env: vec![], inherit_stderr: false, build: None, install_files: vec![] }
                .with_env("YAAC_FAKE_PROVIDER", "1")
                // Never let this test probe or start the user's real harnesses.
                .with_env("YAAC_CODEX_PROVIDER", "0")
                .with_env("YAAC_CLAUDE_PROVIDER", "0")
                .with_env("YAAC_HOME", dir.join(".yaac"))
                .with_env("YAAC_SOCKET", home.dir.path().join(format!("{name}.sock")))
                .with_env("YAAC_IDLE_TIMEOUT_MS", "3000")
                .with_env("TMPDIR", &tmp)
                .with_env("HOME", &dir);
            homes.insert((*name).to_owned(), dir);
            temps.insert((*name).to_owned(), tmp);
            configs.insert((*name).to_owned(), local);
        }
        Some(Self { home, homes, temps, factory: Arc::new(Daemons(configs)) })
    }

    fn config(&self) -> HubConfig {
        let mut config = HubConfig::new(BUILD, self.home.paths(), self.factory.clone());
        config.seed_local_target = false;
        config.test_methods = true;
        config.handle_signals = false;
        config.idle_exit = Some(Duration::from_secs(600));
        config.backoff = yaac_hub::Backoff { initial: Duration::from_millis(50), max: Duration::from_millis(500) };
        config
    }

    /// `<tmp>/yaac-<user>/uploads` of one daemon (created on its first upload).
    fn uploads(&self, name: &str) -> PathBuf {
        let tmp = &self.temps[name];
        let top = std::fs::read_dir(tmp)
            .unwrap()
            .flatten()
            .find(|e| e.file_name().to_string_lossy().starts_with("yaac-"))
            .map(|e| e.path())
            .unwrap_or_else(|| panic!("{name} has no upload folder yet"));
        top.join("uploads")
    }
}

fn connect(c: &Client, name: &str) -> TargetId {
    let target = c.add_target(name).id;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let t = c.targets().into_iter().find(|t| t.id == target).unwrap();
        if t.state.status == ConnectionStatus::Connected {
            return target;
        }
        assert!(Instant::now() < deadline, "the real daemon {name} never connected: {:?}", t.state);
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn large_json_round_trips_through_window_hub_and_daemon() {
    let Some(rig) = Rig::new(&["alpha"]) else { return };
    let hub = rig.home.start_with(rig.config());
    let c = hub.client();
    let target = connect(&c, "alpha");
    let providers = c.routed(&target, "provider/list", json!({})).unwrap();
    let instance = providers["providers"][0]["instance"].as_str().unwrap();
    let prompt = "\"🎨\n".repeat(220_000);
    let started = c.routed(&target, "thread/start", json!({
        "opId": "large-json", "instance": instance, "cwd": rig.homes["alpha"],
        "model": "fake-large", "input": [{ "type": "text", "text": prompt }]
    })).expect("thread start reaches the daemon through both links");
    let thread = &started["threadId"];
    let mut cursor = Value::Null;
    loop {
        let page = c.routed(&target, "thread/read", json!({ "threadId": thread, "cursor": cursor })).unwrap();
        if let Some(user) = page["items"].as_array().unwrap().iter().find(|item| item["kind"] == "userMessage") {
            assert_eq!(user["parts"][0]["text"].as_str(), Some(prompt.as_str()));
            break;
        }
        cursor = page["cursor"].clone();
        assert!(!cursor.is_null(), "expected the user message in the thread's history");
    }
    drop(c);
    hub.shutdown();
}

fn sha(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// Bytes that do not compress or repeat.
fn noise(len: usize, seed: u64) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x.to_le_bytes()[3]
        })
        .collect()
}

fn png(len: usize) -> Vec<u8> {
    let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    bytes.extend(noise(len, 7));
    bytes
}

/// The hub's upload notifications, as they arrive.
#[derive(Default)]
struct Seen(Vec<(String, Value)>);

impl Seen {
    fn pump(&mut self, c: &Client) {
        for m in c.drain() {
            if let Message::Notification { method, params } = m
                && method.starts_with("hub/upload/")
            {
                self.0.push((method, params));
            }
        }
    }

    fn done(&mut self, c: &Client, handle: &str) -> Value {
        let mut found = None;
        wait_until(&format!("hub/upload/done for {handle}"), || {
            self.pump(c);
            found = self.0.iter().find(|(m, p)| m == ext::UPLOAD_DONE && p["handle"] == handle).map(|(_, p)| p.clone());
            found.is_some()
        });
        found.unwrap()
    }

    fn progress(&self, handle: &str) -> Vec<Value> {
        self.0.iter().filter(|(m, p)| m == ext::UPLOAD_PROGRESS && p["handle"] == handle).map(|(_, p)| p.clone()).collect()
    }
}

fn start(c: &Client, params: Value) -> String {
    let r = c.call(ext::UPLOAD_START, params).expect("hub/upload/start");
    r["handle"].as_str().expect("handle").to_owned()
}

fn instance(c: &Client, target: &TargetId) -> String {
    let providers = c.routed(target, "provider/list", json!({})).expect("provider/list");
    providers["providers"][0]["instance"].as_str().expect("a fake provider instance").to_owned()
}

/// The thread's items, oldest first.
fn items(c: &Client, target: &TargetId, thread: &str) -> Vec<Item> {
    let read = c.routed(target, "thread/read", json!({ "threadId": thread })).expect("thread/read");
    let mut items: Vec<Item> = serde_json::from_value(read["items"].clone()).unwrap();
    items.reverse();
    items
}

fn agent_text(items: &[Item]) -> String {
    items
        .iter()
        .filter_map(|i| match &i.body {
            ItemBody::AgentMessage(m) => Some(m.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn files_reach_the_upload_folder_once_and_images_reach_the_model_as_images() {
    let Some(rig) = Rig::new(&["alpha"]) else { return };
    let hub = rig.home.start_with(rig.config());
    let c = hub.client();
    let a = connect(&c, "alpha");
    let mut seen = Seen::default();
    let files = rig.home.dir.path().join("files");
    std::fs::create_dir_all(&files).unwrap();

    // A file: the hub reads it; the target stores it by digest under its own name.
    let log = noise(300 * 1024, 1);
    let log_path = write(&files, "error-log.txt", &log);
    let h1 = start(&c, json!({ "target": a, "draft": "new:alpha", "path": log_path }));
    let done = seen.done(&c, &h1);
    assert!(done.get("error").is_none(), "{done}");
    assert_eq!(done["sha256"], sha(&log), "{done}");
    assert_eq!(done["size"], log.len());
    assert_eq!(done["skipped"], false);
    assert_eq!(done["draft"], "new:alpha");
    let upload_id = done["uploadId"].as_str().unwrap().to_owned();
    let uploads = rig.uploads("alpha");
    assert_eq!(std::fs::read(uploads.join("blobs").join(sha(&log))).unwrap(), log, "the bytes arrived intact");
    let meta: Value = serde_json::from_slice(&std::fs::read(uploads.join("meta").join(format!("{upload_id}.json"))).unwrap()).unwrap();
    assert_eq!(meta["name"], "error-log.txt");
    assert!(seen.progress(&h1).iter().any(|p| p["phase"] == "sending"), "{:?}", seen.progress(&h1));

    // The same content again: no bytes are sent.
    let copy = write(&files, "copy.txt", &log);
    let h2 = start(&c, json!({ "target": a, "path": copy }));
    let again = seen.done(&c, &h2);
    assert_eq!(again["skipped"], true, "{again}");
    assert_ne!(again["uploadId"], upload_id, "a new upload id for the same content");
    assert!(seen.progress(&h2).iter().all(|p| p["phase"] != "sending"), "{:?}", seen.progress(&h2));

    // An image, recognised by content.
    let shot = png(40 * 1024);
    let shot_path = write(&files, "shot.bin", &shot);
    let h3 = start(&c, json!({ "target": a, "path": shot_path, "name": "shot.png" }));
    let image = seen.done(&c, &h3);
    assert_eq!(image["mediaType"], "image/png", "{image}");
    let image_id = image["uploadId"].as_str().unwrap().to_owned();

    // Sent with a model that sees images: the provider gets image input and a file path.
    let inst = instance(&c, &a);
    let cwd = rig.homes["alpha"].display().to_string();
    let started = c
        .routed(&a, "thread/start", json!({
            "opId": "op-wp9-1", "instance": inst, "cwd": cwd, "model": "fake-large",
            "input": [
                { "type": "text", "text": "/fake inputs" },
                { "type": "image", "uploadId": image_id },
                { "type": "file", "uploadId": upload_id }
            ]
        }))
        .expect("thread/start with an image");
    let thread = started["threadId"].as_str().unwrap().to_owned();
    let mut got = Vec::new();
    // The report streams in deltas: wait for its last line.
    wait_until("the fake reports its inputs", || {
        got = items(&c, &a, &thread);
        agent_text(&got).contains("error-log.txt error-log.txt")
    });
    let text = agent_text(&got);
    assert!(text.contains("-shot.png image/png"), "the image arrived as image input: {text}");
    assert!(text.contains("-error-log.txt error-log.txt"), "the file arrived as a path: {text}");
    // The journaled message keeps each part's name and media type, for chips on any host.
    let parts = got
        .iter()
        .find_map(|i| match &i.body {
            ItemBody::UserMessage(m) => Some(m.parts.clone()),
            _ => None,
        })
        .expect("the user message");
    let named: Vec<(String, Option<String>)> = parts
        .iter()
        .filter_map(|p| match p {
            InputPart::Image(i) => Some((i.name.clone().unwrap_or_default(), i.media_type.clone())),
            InputPart::File(f) => Some((f.name.clone().unwrap_or_default(), f.media_type.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(named, vec![("shot.png".to_owned(), Some("image/png".to_owned())), ("error-log.txt".to_owned(), None)]);

    // A model that cannot see images: the target refuses the image part.
    let refused = c
        .routed(&a, "thread/start", json!({
            "opId": "op-wp9-2", "instance": inst, "cwd": cwd, "model": "fake-small",
            "input": [{ "type": "text", "text": "look" }, { "type": "image", "uploadId": image_id }]
        }))
        .expect_err("an image for a model without image input");
    assert_eq!(refused.code.as_str(), "capability_missing", "{refused:?}");
    // As a file it is fine.
    c.routed(&a, "thread/start", json!({
        "opId": "op-wp9-3", "instance": inst, "cwd": cwd, "model": "fake-small",
        "input": [{ "type": "text", "text": "look" }, { "type": "file", "uploadId": image_id }]
    }))
    .expect("the image as a file");

    // Errors a window shows as-is.
    let missing = start(&c, json!({ "target": a, "path": files.join("nope.txt") }));
    assert_eq!(seen.done(&c, &missing)["error"]["code"], "file_unreadable");
    assert!(c.call(ext::UPLOAD_START, json!({ "target": a, "path": "relative.txt" })).is_err());
    assert!(c.call(ext::UPLOAD_START, json!({ "target": "t_nope", "path": log_path })).is_err());
    assert!(c.call(ext::UPLOAD_LIST, json!({})).unwrap()["uploads"].as_array().unwrap().is_empty());
    drop(c);
    hub.shutdown();
}

#[test]
fn a_large_upload_leaves_control_prompt_and_cancel_removes_the_partial_file() {
    let Some(rig) = Rig::new(&["alpha"]) else { return };
    let hub = rig.home.start_with(rig.config());
    let c = hub.client();
    let a = connect(&c, "alpha");
    let mut seen = Seen::default();
    let files = rig.home.dir.path().join("files");
    std::fs::create_dir_all(&files).unwrap();

    // 20 MB, while control requests go back and forth.
    let big = noise(20 * 1024 * 1024, 2);
    let big_path = write(&files, "big.bin", &big);
    let h = start(&c, json!({ "target": a, "path": big_path }));
    let mut during: Vec<Duration> = Vec::new();
    let finished = loop {
        seen.pump(&c);
        if let Some((_, done)) = seen.0.iter().find(|(m, p)| m == ext::UPLOAD_DONE && p["handle"] == h.as_str()) {
            break done.clone();
        }
        let sending = seen.progress(&h).last().is_some_and(|p| p["phase"] == "sending");
        let t0 = Instant::now();
        c.routed(&a, "ping", json!({})).expect("ping during the upload");
        let rtt = t0.elapsed();
        // Count only round trips that started and ended while bytes were moving.
        seen.pump(&c);
        let still = !seen.0.iter().any(|(m, p)| m == ext::UPLOAD_DONE && p["handle"] == h.as_str());
        if sending && still {
            during.push(rtt);
        }
    };
    assert!(finished.get("error").is_none(), "{finished}");
    assert_eq!(finished["sha256"], sha(&big));
    assert_eq!(std::fs::metadata(rig.uploads("alpha").join("blobs").join(sha(&big))).unwrap().len(), big.len() as u64);
    during.sort();
    let max = *during.last().expect("at least one control round trip while the upload was sending");
    let median = during[during.len() / 2];
    eprintln!("control round trips during a 20 MB upload: n={} median={median:?} max={max:?}", during.len());
    // A timing gate: checked on a developer machine, only reported in CI, whose runners are
    // too variable for wall-clock budgets.
    eprintln!("slowest control round trip during the upload: {max:?}");
    if std::env::var_os("CI").is_none() {
        assert!(max < Duration::from_millis(500), "a control round trip took {max:?} during the upload");
    }

    // Cancel in the middle: the target removes the partial file.
    let huge = noise(96 * 1024 * 1024, 3);
    let huge_path = write(&files, "huge.bin", &huge);
    drop(huge);
    let h = start(&c, json!({ "target": a, "path": huge_path }));
    wait_until("bytes moving", || {
        seen.pump(&c);
        seen.progress(&h).iter().any(|p| p["phase"] == "sending" && p["sent"].as_u64().unwrap_or(0) > 0)
            || seen.0.iter().any(|(m, p)| m == ext::UPLOAD_DONE && p["handle"] == h.as_str())
    });
    let partial = rig.uploads("alpha").join("partial");
    c.call(ext::UPLOAD_CANCEL, json!({ "handle": h })).expect("cancel");
    let done = seen.done(&c, &h);
    assert_eq!(done["error"]["code"], "upload_cancelled", "{done}");
    wait_until("the partial file removed", || std::fs::read_dir(&partial).unwrap().next().is_none());
    let blobs: Vec<String> = std::fs::read_dir(rig.uploads("alpha").join("blobs")).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(blobs, vec![sha(&big)], "nothing of the cancelled upload was stored");
    // The connection is still good.
    c.routed(&a, "ping", json!({})).expect("ping after the cancel");
    drop(c);
    hub.shutdown();
}

#[test]
fn a_drafts_clipboard_image_survives_a_hub_restart_and_follows_a_retarget() {
    let Some(rig) = Rig::new(&["alpha", "beta"]) else { return };
    let shot = png(1300 * 1024); // more than two chunks
    let blob_id;
    {
        let mut config = rig.config();
        config.uploads.max_blob = 2 * 1024 * 1024;
        config.uploads.max_file = 2 * 1024 * 1024;
        let hub = rig.home.start_with(config);
        let c = hub.client();
        let begun = c
            .call(ext::BLOB_BEGIN, json!({ "draft": "new:alpha", "name": "screenshot.png", "mediaType": "image/png", "size": shot.len() }))
            .expect("blob begin");
        blob_id = begun["blobId"].as_str().unwrap().to_owned();
        let mut chunks = 0;
        for chunk in BlobAppendParams::chunks(&blob_id, &shot) {
            c.call(ext::BLOB_APPEND, serde_json::to_value(chunk).unwrap()).expect("append");
            chunks += 1;
        }
        assert_eq!(chunks, 3);
        let ended = c.call(ext::BLOB_END, json!({ "blobId": blob_id })).expect("blob end");
        assert_eq!(ended["sha256"], sha(&shot));
        // Limits, with a message to show.
        let refused = c.call(ext::BLOB_BEGIN, json!({ "draft": "d", "name": "x.png", "size": 3 * 1024 * 1024 })).unwrap_err();
        assert_eq!(refused.code.as_str(), "limit_exceeded");
        assert!(refused.message.contains("2 MiB"), "{}", refused.message);
        // A blob still being written is not usable, and is dropped by a restart.
        let open = c.call(ext::BLOB_BEGIN, json!({ "draft": "new:alpha", "name": "half.png", "size": 10 })).unwrap();
        c.call(ext::BLOB_APPEND, json!({ "blobId": open["blobId"], "offset": 0, "dataBase64": "AAAA" })).unwrap();
        drop(c);
        hub.shutdown();
    }

    // A new hub: the bytes are still there.
    let mut config = rig.config();
    config.uploads.max_file = 2 * 1024 * 1024;
    let hub = rig.home.start_with(config);
    let c = hub.client();
    let listed = c.call(ext::BLOB_LIST, json!({ "draft": "new:alpha" })).unwrap();
    let blobs = listed["blobs"].as_array().unwrap();
    assert_eq!(blobs.len(), 1, "{listed}");
    assert_eq!(blobs[0]["blobId"], blob_id.as_str());
    use base64::Engine as _;
    let mut preview = Vec::new();
    loop {
        let chunk = c.call(ext::BLOB_READ, json!({ "blobId": blob_id, "offset": preview.len() })).unwrap();
        assert_eq!(chunk["present"], true);
        assert_eq!(chunk["mediaType"], "image/png");
        let bytes = base64::engine::general_purpose::STANDARD.decode(chunk["dataBase64"].as_str().unwrap()).unwrap();
        assert!(bytes.len() <= 512 * 1024);
        preview.extend(bytes);
        if chunk["more"] == false {
            break;
        }
    }
    assert_eq!(preview, shot);
    let a = connect(&c, "alpha");
    let b = connect(&c, "beta");
    let mut seen = Seen::default();

    let up_a = seen.done(&c, &start(&c, json!({ "target": a, "draft": "new:alpha", "blobId": blob_id })));
    assert!(up_a.get("error").is_none(), "{up_a}");
    assert_eq!((up_a["name"].as_str(), up_a["mediaType"].as_str(), up_a["skipped"].as_bool()), (Some("screenshot.png"), Some("image/png"), Some(false)));
    assert_eq!(std::fs::read(rig.uploads("alpha").join("blobs").join(sha(&shot))).unwrap(), shot);

    // The draft's target changes: the window asks again, and the bytes go to the new target.
    let up_b = seen.done(&c, &start(&c, json!({ "target": b, "draft": "new:alpha", "blobId": blob_id })));
    assert!(up_b.get("error").is_none(), "{up_b}");
    assert_eq!(up_b["skipped"], false, "beta did not have it: bytes were sent");
    assert_eq!(std::fs::read(rig.uploads("beta").join("blobs").join(sha(&shot))).unwrap(), shot);
    // An upload id means nothing on another target.
    let inst_b = instance(&c, &b);
    let wrong = c
        .routed(&b, "thread/start", json!({
            "opId": "op-wp9-4", "instance": inst_b, "cwd": rig.homes["beta"].display().to_string(),
            "input": [{ "type": "text", "text": "x" }, { "type": "image", "uploadId": up_a["uploadId"] }]
        }))
        .expect_err("alpha's upload id on beta");
    assert_eq!(wrong.code.as_str(), "upload_not_found");
    // Back to the first target: content addressing makes it free.
    let again = seen.done(&c, &start(&c, json!({ "target": a, "blobId": blob_id })));
    assert_eq!(again["skipped"], true, "{again}");

    // A file over the limit is refused with a message to show.
    let files = rig.home.dir.path().join("files");
    std::fs::create_dir_all(&files).unwrap();
    let big = write(&files, "big.bin", &noise(3 * 1024 * 1024, 4));
    let over = seen.done(&c, &start(&c, json!({ "target": a, "path": big })));
    assert_eq!(over["error"]["code"], "limit_exceeded", "{over}");
    assert!(over["error"]["message"].as_str().unwrap().contains("big.bin"), "{over}");

    // The message is sent: deleting the draft deletes its bytes.
    c.call(ext::DRAFTS_DELETE, json!({ "key": "new:alpha" })).unwrap();
    assert!(c.call(ext::BLOB_LIST, json!({})).unwrap()["blobs"].as_array().unwrap().is_empty());
    let gone = seen.done(&c, &start(&c, json!({ "target": a, "blobId": blob_id })));
    assert_eq!(gone["error"]["code"], "blob_not_found", "{gone}");
    let left: Vec<_> = std::fs::read_dir(rig.home.paths().data_dir.join("draft-blobs")).unwrap().flatten().collect();
    assert!(left.is_empty(), "blob files left: {left:?}");
    drop(c);
    hub.shutdown();
}
