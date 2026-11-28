//! Attachments end to end, without a window: a real `yaac hub` runs headless in a temporary
//! profile and starts the real daemon bundle on its fake provider (`YAAC_FAKE_PROVIDER=1`, the
//! real harnesses switched off); the window's own client attaches, names a file to the hub
//! (`hub/upload/start`), stores clipboard bytes as a draft blob, and sends both in the first
//! message of a thread. The fake provider's `/fake inputs` reports what it was handed.
//!
//! Runs only when `daemon/dist/yaacd.mjs` exists and `node` (or `YAAC_RUNTIME`) runs;
//! otherwise it prints that it was skipped and passes.
#![cfg(all(unix, feature = "hub"))]

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use yaac_app::attach::{AttachConfig, HubAttach, SharedAttach};
use yaac_core::attachments::{DraftRef, Upload};
use yaac_core::client::Client;
use yaac_hub::HubPaths;
use yaac_protocol::{ClientId, ItemBody, ProviderStatus};

const WAIT: Duration = Duration::from_secs(60);

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn runnable() -> bool {
    if !root().join("daemon/dist/yaacd.mjs").is_file() {
        eprintln!("SKIPPED: daemon/dist/yaacd.mjs does not exist (build the daemon to run this test)");
        return false;
    }
    let runtime = std::env::var_os("YAAC_RUNTIME").unwrap_or_else(|| "node".into());
    if Command::new(&runtime).arg("--version").output().is_err() {
        eprintln!("SKIPPED: the runtime {runtime:?} does not run");
        return false;
    }
    true
}

fn pump_until(c: &mut Client, what: &str, done: impl Fn(&Client) -> bool) {
    let deadline = Instant::now() + WAIT;
    while !done(c) {
        assert!(Instant::now() < deadline, "timed out waiting for {what}: {:?} / {:?}", c.state.hub, c.errors);
        c.pump();
        std::thread::sleep(Duration::from_millis(10));
    }
}

struct Hub {
    child: Child,
}

impl Drop for Hub {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn png(len: usize) -> Vec<u8> {
    let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let mut x: u32 = 0x1234_5678;
    bytes.extend((0..len).map(|_| {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        x.to_le_bytes()[1]
    }));
    bytes
}

fn agent_text(c: &Client) -> String {
    c.state
        .transcripts
        .values()
        .flat_map(|t| t.items().iter())
        .filter_map(|i| match &i.body {
            ItemBody::AgentMessage(m) => Some(m.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn a_file_and_a_clipboard_image_reach_the_fake_provider_through_the_real_hub() {
    if !runnable() {
        return;
    }
    // Under /tmp, so the socket paths stay short.
    let dir = tempfile::Builder::new().prefix("ya").tempdir_in("/tmp").expect("temp dir");
    let var = |n: &str| dir.path().join(n);
    for d in ["home", "tmp", "files"] {
        std::fs::create_dir_all(var(d)).unwrap();
    }
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_yaac"));
    // No settings file: the hub seeds this computer as its one target.
    let child = Command::new(&exe)
        .arg("hub")
        .env("YAAC_CONFIG_DIR", var("config"))
        .env("YAAC_DATA_DIR", var("data"))
        .env("YAAC_RUNTIME_DIR", var("run"))
        .env("YAAC_ROOT", root())
        // The daemon inherits these: the fake provider only, its own home, socket and temp.
        .env("YAAC_FAKE_PROVIDER", "1")
        .env("YAAC_CODEX_PROVIDER", "0")
        .env("YAAC_CLAUDE_PROVIDER", "0")
        .env("YAAC_HOME", var("home").join(".yaac"))
        .env("YAAC_SOCKET", var("d.sock"))
        .env("YAAC_IDLE_TIMEOUT_MS", "5000")
        .env("TMPDIR", var("tmp"))
        .env("HOME", var("home"))
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("yaac hub");
    let _hub = Hub { child };
    let paths = {
        let (c, d, r) = (var("config"), var("data"), var("run"));
        HubPaths::from_lookup(move |k| match k {
            "YAAC_CONFIG_DIR" => Some(c.clone().into_os_string()),
            "YAAC_DATA_DIR" => Some(d.clone().into_os_string()),
            "YAAC_RUNTIME_DIR" => Some(r.clone().into_os_string()),
            _ => None,
        })
        .expect("paths")
    };
    let deadline = Instant::now() + WAIT;
    while !paths.socket().exists() {
        assert!(Instant::now() < deadline, "the hub did not listen");
        std::thread::sleep(Duration::from_millis(20));
    }
    let config = AttachConfig { paths, exe, autostart: None, start_timeout: Duration::from_secs(15), remote: None, bridge: None };
    let a = Arc::new(Mutex::new(HubAttach::start(config, Arc::new(|| {}))));
    let mut c = Client::new(Box::new(SharedAttach(a)), yaac_app::BUILD, ClientId::new("test-window"));
    pump_until(&mut c, "attached", |c| c.state.hub.attached);
    pump_until(&mut c, "the fake provider on this computer", |c| {
        c.state.targets.iter().any(|t| t.connected() && t.providers.iter().any(|p| p.status == ProviderStatus::Ready))
    });
    let target = c.state.targets.iter().find(|t| t.connected()).unwrap();
    let target_id = target.entry.id.clone();
    let provider = target.providers.iter().find(|p| p.status == ProviderStatus::Ready).unwrap();
    let instance = provider.instance.clone();
    // A model that sees images (the fake provider's own names; nothing in the window knows them).
    let model = provider.models.iter().find(|m| m.id.as_str() == "fake-large").map(|m| m.id.clone()).expect("fake-large");

    // A new thread on this computer, in the daemon's home.
    let cwd = var("home").to_string_lossy().into_owned();
    let draft = c.new_thread(target_id.clone(), Some(cwd.into()));
    if let Some(d) = c.state.new_threads.iter_mut().find(|d| d.id == draft) {
        d.instance = Some(instance);
        d.model = Some(model);
    }
    let d = DraftRef::New(draft);
    // A file, named to the hub: it reads and hashes it.
    let log = var("files").join("error-log.txt");
    std::fs::write(&log, "line 1\nline 2\n").unwrap();
    c.attach_files(&d, &[log.to_string_lossy().into_owned()]);
    // A clipboard image: stored in the hub's draft store as a blob first.
    let shot = png(40 * 1024);
    let chunks: Vec<String> = {
        use base64::Engine as _;
        shot.chunks(yaac_core::attachments::BLOB_CHUNK_MAX).map(|ch| base64::engine::general_purpose::STANDARD.encode(ch)).collect()
    };
    c.attach_bytes(&d, "screenshot-2026-11-20-1405.png", Some("image/png"), shot.len() as u64, chunks);
    pump_until(&mut c, "both uploads done", |c| {
        let atts = &c.draft(&d).unwrap().attachments;
        if let Some(Upload::Failed { message, .. }) = atts.iter().map(|a| &a.upload).find(|u| matches!(u, Upload::Failed { .. })) {
            panic!("an upload failed: {message}");
        }
        atts.len() == 2 && atts.iter().all(|a| a.ready_on(Some(&target_id)).is_some())
    });
    let atts = c.draft(&d).unwrap().attachments.clone();
    assert!(atts[1].is_image(), "the hub sniffed the clipboard bytes as an image: {:?}", atts[1].media_type);

    // Sent in the first message.
    if let Some(x) = c.state.new_threads.iter_mut().find(|x| x.id == draft) {
        x.draft.text = "/fake inputs".into();
    }
    assert!(c.start_thread(draft), "sendable once both uploads are done");
    pump_until(&mut c, "the fake provider's report of its inputs", |c| agent_text(c).contains("error-log.txt error-log.txt"));
    let text = agent_text(&c);
    assert!(text.contains("-error-log.txt error-log.txt"), "the file arrived as a path: {text}");
    assert!(text.contains("screenshot-2026-11-20-1405.png image/png"), "the image arrived as image input: {text}");
    c.shutdown_hub();
    pump_until(&mut c, "shutdown", |c| c.state.hub.shutdown_answered);
}
